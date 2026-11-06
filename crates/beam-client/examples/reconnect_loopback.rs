//! 真机验证（§4.1）：客户端**换一条新连接**。
//!
//! ## 它证明什么
//!
//! B25 之前，客户端的「重连」在真实链路上是**残废**的：
//!
//! - 会话层的 `TransportFactory::connect()` 是**同步**接口，而建 QUIC 连接是**异步**的
//!   ⇒ 调用方只能在同步上下文里 `block_on`，在 tokio 工作线程上会卡住运行时；
//! - 桌面端用的 `ReadyTransportFactory` 只能交出**同一条**已建好的连接
//!   ⇒ §4.1 的「新连接/迁移」重连根本换不了连接（状态机跑得再对也没用）。
//!
//! 本入口用**可重建**的工厂（[`beam_client::NetTransportFactory`] + tokio 运行时句柄）
//! 真机跑两个阶段：
//!
//! | 阶段 | 场景 | 期望 |
//! |---|---|---|
//! | A | 客户端连未信任的主机（**不传 PIN**）⇒ 主机回 E3008「还没配对好」；2.5 s 后模拟「用户在主机上点同意」 | 客户端**自己重建连接**并进串流；**建链次数 ≥ 2**（证明真的换了连接） |
//! | B | 串流中主机**掐断**连接（模拟掉线） | 客户端 3 s 停滞判定 → 重建**新**连接 → `Resume` → 回到串流；建链次数再 +1 |
//!
//! ## 用法
//!
//! ```powershell
//! cargo run -p beam-client --example reconnect_loopback --release
//! ```
//!
//! 不需要 ffmpeg：这个入口只验**控制面**（连接重建 / 配对等待 / 恢复），不碰解码与上屏。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_client::{ClientConfig, ClientRuntime, NetTransportFactory};
use beam_decode::D3D11VaDecoder;
use beam_host::{HostConfig, HostListener, HostRuntime, HostTrustGate};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_render::D3D11Renderer;
use beam_session::{PeerAddr, PendingTransport, SessionEvent, SessionState, TransportFactory};
use futures::{FutureExt, StreamExt};

/// 主机显示的 PIN（§3.3 `pin_len=4`）。
const HOST_PIN: &str = "1234";
/// 阶段 A：多久之后模拟「用户在主机上点同意」。
const APPROVE_AFTER: Duration = Duration::from_millis(2500);
/// 每个阶段的等待上限。
const PHASE_TIMEOUT: Duration = Duration::from_secs(25);

/// 计数包装：记录**发起过几次建链**。
///
/// 这是本入口最关键的判据：会话层每重建一次连接就会调一次
/// [`TransportFactory::start_connect`]，所以它就是「换了几条连接」的真实计数。
struct CountingFactory {
    inner: NetTransportFactory,
    connects: Arc<AtomicUsize>,
}

impl TransportFactory for CountingFactory {
    fn start_connect(&mut self) -> Box<dyn PendingTransport> {
        let n = self.connects.fetch_add(1, Ordering::SeqCst) + 1;
        println!("    [客户端] 第 {n} 次发起建链（异步，不阻塞会话）");
        self.inner.start_connect()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("=== 真机验证（§4.1）：客户端换一条**新**连接 ===");

    // ---------------------------------------------------------------- 身份
    let host_identity = Identity::generate()?;
    let client_identity = Identity::generate()?;
    let host_fp = host_identity.fingerprint();
    let client_fp = client_identity.fingerprint();
    println!("主机指纹 = {}", host_fp.short_hex());
    println!("客户端指纹 = {}", client_fp.short_hex());

    // 主机信任库：开局是**空的**（客户端还没被用户确认过）。
    let trust = Arc::new(Mutex::new(TrustStore::new()));
    // 闸门进**配对模式**：未信任对端的握手也放行（控制面再把关，§3.3）。
    let gate = Arc::new(HostTrustGate::new(Arc::clone(&trust)));
    gate.set_pairing_mode(true);

    // 客户端信任库：必须先有主机的指纹（TLS 校验主机证书）。
    let mut client_trust = TrustStore::new();
    client_trust.upsert(TrustedPeer::new(host_fp, "回环主机", "windows", 0));

    // ---------------------------------------------------------------- 监听
    let listen_cfg = beam_net::NetConfig {
        identity: Some(beam_net::TlsIdentity {
            cert_chain: host_identity.certificate_chain(),
            key: host_identity.private_key(),
        }),
        listen_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..beam_net::NetConfig::default()
    };
    let listener = HostListener::listen(
        listen_cfg,
        Arc::clone(&gate) as Arc<dyn beam_pair::PeerVerifier>,
    )
    .await?;
    let addr = listener.local_addr()?;
    println!("主机监听地址 = {addr}");

    // 主机接受循环（后台）：记连接数，并把「当前连接」放进槽子供阶段 B 掐断。
    let accepted = Arc::new(AtomicUsize::new(0));
    let current_conn: Arc<Mutex<Option<beam_net::Conn>>> = Arc::new(Mutex::new(None));
    let host_task = tokio::spawn(host_accept_loop(
        Arc::new(listener),
        Arc::clone(&gate),
        Arc::clone(&trust),
        host_fp,
        Arc::clone(&accepted),
        Arc::clone(&current_conn),
    ));

    // ---------------------------------------------------------------- 客户端
    //
    // 关键：用**可重建**的工厂（`NetTransportFactory` + tokio 运行时句柄）。
    // 它不在 tokio 运行时里 `block_on`，而是把建链**发起**到运行时上，
    // 会话层在 `pump` 里非阻塞地问结果 ⇒ 重建连接不再卡运行时。
    let net_cfg = beam_net::NetConfig {
        identity: Some(beam_net::TlsIdentity {
            cert_chain: client_identity.certificate_chain(),
            key: client_identity.private_key(),
        }),
        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..beam_net::NetConfig::default()
    };
    let connects = Arc::new(AtomicUsize::new(0));
    let factory = CountingFactory {
        inner: NetTransportFactory::new(
            addr,
            net_cfg,
            Arc::new(client_trust.clone()) as Arc<dyn beam_net::PeerVerifier>,
        )
        .with_runtime(tokio::runtime::Handle::current()),
        connects: Arc::clone(&connects),
    };

    let mut cfg = ClientConfig::from_resolved(&beam_config::ResolvedConfig::default());
    // 阶段 A：**不传 PIN** ⇒ 客户端不发起配对 ⇒ 主机回 E3008（等人工确认）。
    cfg.pin = None;
    cfg.fingerprint = Some(client_identity.fingerprint());

    let runtime = ClientRuntime::new(cfg);
    let peer = PeerAddr {
        host: addr.ip().to_string(),
        port: addr.port(),
        fingerprint: None,
    };
    let handle = runtime.connect(
        peer,
        D3D11VaDecoder::new(),
        D3D11Renderer::new(),
        Some(Box::new(factory)),
    );
    let mut events = handle.events();

    // ---------------------------------------------------------------- 阶段 A
    println!("\n[A] 客户端连未信任的主机（不传 PIN）⇒ 主机应回 E3008「还没配对好」");
    let deadline = Instant::now() + PHASE_TIMEOUT;
    let approve_at = Instant::now() + APPROVE_AFTER;
    let mut approved = false;
    let mut saw_pending = false;
    let mut state = handle.state();
    while Instant::now() < deadline {
        let _ = handle.pump();
        state = handle.state();
        while let Some(Some(event)) = events.next().now_or_never() {
            if let SessionEvent::Error { code, .. } = event {
                if code == beam_session::PAIRING_PENDING_CODE {
                    saw_pending = true;
                    println!(
                        "    [客户端] 收到 E3008「还没配对好」⇒ 进入等待并按 1 s 间隔重建连接"
                    );
                }
            }
        }
        if !approved && Instant::now() >= approve_at {
            approved = true;
            // 模拟「用户在主机上点同意」：把客户端指纹写进信任库。
            {
                let mut guard = trust.lock().unwrap_or_else(|e| e.into_inner());
                guard.upsert(TrustedPeer::new(client_fp, "回环客户端", "windows", 0));
            }
            println!("    [主机] 用户点了「同意」⇒ 客户端指纹已写进信任库");
        }
        if state == SessionState::Streaming || state == SessionState::Error {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let connects_a = connects.load(Ordering::SeqCst);
    let accepted_a = accepted.load(Ordering::SeqCst);
    println!("    客户端状态 = {state}");
    println!("    客户端发起建链次数 = {connects_a}；主机接受连接次数 = {accepted_a}");
    println!("    收到过 E3008 = {saw_pending}");
    // 判据：① 进了串流；② 收到过 E3008；③ **建链次数 ≥ 2**（真的换了连接）。
    let phase_a_ok = state == SessionState::Streaming && saw_pending && connects_a >= 2;

    // ---------------------------------------------------------------- 阶段 B
    println!("\n[B] 主机掐断当前连接 ⇒ 客户端应重建**新**连接并恢复串流");
    {
        let guard = current_conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.as_ref() {
            // 直接关连接（不发 `Bye`）：模拟真实网络掉线。
            conn.close(0, b"simulated-drop");
            println!("    [主机] 已掐断连接");
        } else {
            println!("    [主机] 当前没有连接（阶段 A 未成功？）");
        }
    }
    let connects_before = connects.load(Ordering::SeqCst);
    let deadline = Instant::now() + PHASE_TIMEOUT;
    let mut resumed = false;
    while Instant::now() < deadline {
        let _ = handle.pump();
        state = handle.state();
        if state == SessionState::Streaming && connects.load(Ordering::SeqCst) > connects_before {
            resumed = true;
            break;
        }
        if state == SessionState::Error || state == SessionState::Closed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let connects_b = connects.load(Ordering::SeqCst);
    let accepted_b = accepted.load(Ordering::SeqCst);
    println!("    客户端状态 = {state}");
    println!("    客户端发起建链次数 = {connects_b}；主机接受连接次数 = {accepted_b}");
    let phase_b_ok = resumed && connects_b > connects_before;

    host_task.abort();

    // ---------------------------------------------------------------- 结论
    println!("--------------------------------------------------");
    println!(
        "阶段 A 等确认后自动连上 : {}",
        if phase_a_ok { "通过" } else { "未通过" }
    );
    println!(
        "阶段 B 掉线后换新连接   : {}",
        if phase_b_ok { "通过" } else { "未通过" }
    );
    if phase_a_ok && phase_b_ok {
        println!("RESULT: PASS —— 客户端真的能**换一条新连接**（§4.1 新连接/迁移不再是残废的）");
        Ok(())
    } else {
        Err(format!("RESULT: FAIL —— 阶段 A {phase_a_ok} / 阶段 B {phase_b_ok}").into())
    }
}

/// 主机接受循环：每次接受连接都起一个主机会话并推进它。
async fn host_accept_loop(
    listener: Arc<HostListener>,
    gate: Arc<HostTrustGate>,
    trust: Arc<Mutex<TrustStore>>,
    host_fp: beam_pair::Fingerprint,
    accepted: Arc<AtomicUsize>,
    current_conn: Arc<Mutex<Option<beam_net::Conn>>>,
) {
    loop {
        match listener.accept().await {
            Ok(conn) => {
                let n = accepted.fetch_add(1, Ordering::SeqCst) + 1;
                println!("    [主机] 接受第 {n} 条连接");
                // 阶段 B 用它掐断连接（模拟掉线）。
                {
                    let mut guard = current_conn.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = Some(conn.clone());
                }
                // §3.3：取走这次握手的信任判定 ⇒ 要不要要求配对。
                let verdict = gate.take_verdict();
                let pairing = beam_host::HostPairing::for_verdict(
                    verdict,
                    Some(HOST_PIN.to_string()),
                    None,
                    Some(host_fp),
                );
                let require_pairing = pairing.require_pairing;
                let runtime = HostRuntime::new(HostConfig::from_resolved(
                    &beam_config::ResolvedConfig::default(),
                ));
                let (pipeline, errors) = beam_host::default_frame_pipeline(runtime.config());
                let input = beam_host::NoopInputSink::new();
                let handle = match runtime
                    .start_quic_session_with_pipeline(conn, pipeline, errors, Some(input), pairing)
                    .await
                {
                    Ok(handle) => handle,
                    Err(err) => {
                        tracing::warn!(reason = %err, "主机会话建立失败");
                        continue;
                    }
                };
                // §3.3：配对成功 ⇒ 把对端写进信任库（与桌面端同一份实现）。
                handle.spawn_trust_writer(Arc::clone(&trust));
                println!("    [主机] 本次会话要求配对 = {require_pairing}");
                // 推进会话：控制消息（配对 / Hello）要靠 pump 才被处理。
                tokio::spawn(async move {
                    loop {
                        if handle.pump().is_err() {
                            break;
                        }
                        if handle.session.is_stopped() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                });
            }
            Err(err) => {
                tracing::debug!(reason = %err, "接受连接失败");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}
