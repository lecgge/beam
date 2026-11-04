//! 真机配对验证（§3.3）：主机显示 PIN → 客户端输入 → **SPAKE2 校验** → 写信任库 → 进串流。
//!
//! ## 为什么要有它
//!
//! 此前 PIN 只到「格式校验 + 尝试预算」这一步（桌面端 `pin.rs`）：
//! **密码学校验从来没有真正发生** —— 主机侧没把 PIN 配进 `PairingConfig`，
//! 信任闸门又在 TLS 层就把未信任对端拒了，客户端连控制通道都到不了。
//!
//! 本入口把 §3.3 的整条路**真机跑一遍**，三个阶段：
//!
//! | 阶段 | 客户端输入 | 期望 |
//! |---|---|---|
//! | 1 | **错的** PIN | 主机回 `PairFail E3001`；客户端进 Error，**进不了串流**；主机信任库仍然为空 |
//! | 2 | **对的** PIN | SPAKE2 交换成功 → `PairOk` → 主机把对端指纹**写进信任库** → 双方进串流 |
//! | 3 | **空** PIN | 对端此时已在信任库里 ⇒ 主机**不再要求配对** ⇒ 直接进串流（§3.3「已配对的主机可传空串」） |
//!
//! 阶段 3 是关键：它证明「配对」不是一次性把戏，而是真的落进了信任库。
//!
//! ## 用法
//!
//! ```powershell
//! cargo run -p beam-client --example pairing_loopback --release
//! ```
//!
//! 不需要 ffmpeg：这个入口只验**控制面**（配对 + 能力协商），不碰解码与上屏。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_client::{ClientConfig, ClientRuntime, ReadyTransportFactory};
use beam_decode::D3D11VaDecoder;
use beam_host::{HostConfig, HostListener, HostRuntime, HostTrustGate};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_render::D3D11Renderer;
use beam_session::{PeerAddr, SessionState, Transport};

/// 主机显示的 PIN（§3.3 `pin_len=4`）。
const HOST_PIN: &str = "1234";
/// 客户端故意输错的 PIN（阶段 1）。
const WRONG_PIN: &str = "9999";
/// 每个阶段等多久。
const PHASE_TIMEOUT: Duration = Duration::from_secs(6);

/// 一个客户端尝试的结果。
struct Attempt {
    /// 最终会话状态。
    state: SessionState,
    /// 是否到达 `Streaming`。
    streaming: bool,
    /// 收到的错误文案（如果有）。
    error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("=== 真机配对验证（§3.3）：PIN → SPAKE2 → 信任库 → 串流 ===");

    // ---------------------------------------------------------------- 身份
    let host_identity = Identity::generate()?;
    let client_identity = Identity::generate()?;
    let host_fp = host_identity.fingerprint();
    let client_fp = client_identity.fingerprint();
    println!("主机指纹 = {}", host_fp.short_hex());
    println!("客户端指纹 = {}", client_fp.short_hex());

    // 主机信任库：开局是**空的**（客户端还没配对过）。
    let trust = Arc::new(Mutex::new(TrustStore::new()));
    // 闸门进**配对模式**：未信任对端的握手也放行（控制面再把关，§3.3）。
    let gate = Arc::new(HostTrustGate::new(Arc::clone(&trust)));
    gate.set_pairing_mode(true);

    // 客户端信任库：必须先有主机的指纹（TLS 校验主机证书）。
    let mut client_trust = TrustStore::new();
    client_trust.upsert(TrustedPeer::new(host_fp, "回环配对主机", "windows", 0));

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

    // 主机接受循环（后台）。
    let host_task = tokio::spawn(host_accept_loop(
        Arc::new(listener),
        Arc::clone(&gate),
        Arc::clone(&trust),
        host_fp,
    ));

    // ---------------------------------------------------------------- 阶段 1
    println!("\n[1/3] 客户端输入**错的** PIN（{WRONG_PIN}）⇒ 应当被拒绝");
    let wrong = client_attempt(addr, WRONG_PIN, &client_identity, &client_trust).await?;
    println!("      客户端状态 = {}", wrong.state);
    println!(
        "      客户端收到的错误 = {}",
        wrong.error.as_deref().unwrap_or("无")
    );
    let trust_len = trust
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .peers()
        .len();
    println!("      主机信任库条目数 = {trust_len}");
    let phase1_ok = !wrong.streaming && trust_len == 0;

    // ---------------------------------------------------------------- 阶段 2
    println!("\n[2/3] 客户端输入**对的** PIN（{HOST_PIN}）⇒ 应当配对成功并进串流");
    let right = client_attempt(addr, HOST_PIN, &client_identity, &client_trust).await?;
    println!("      客户端状态 = {}", right.state);
    let peers: Vec<beam_pair::TrustedPeer> = {
        let guard = trust.lock().unwrap_or_else(|e| e.into_inner());
        guard.peers().to_vec()
    };
    println!("      主机信任库条目数 = {}", peers.len());
    let paired = peers.iter().any(|p| p.fp == client_fp);
    println!(
        "      信任库里含客户端指纹 = {paired}（客户端指纹 = {}）",
        client_fp.short_hex()
    );
    let phase2_ok = right.streaming && paired;

    // ---------------------------------------------------------------- 阶段 3
    println!("\n[3/3] 客户端**不传** PIN（已配对）⇒ 主机应当免配对直接放行");
    let trusted = client_attempt(addr, "", &client_identity, &client_trust).await?;
    println!("      客户端状态 = {}", trusted.state);
    let phase3_ok = trusted.streaming;

    host_task.abort();

    // ---------------------------------------------------------------- 结论
    println!("--------------------------------------------------");
    println!(
        "阶段 1 错的 PIN 被拒     : {}",
        if phase1_ok { "通过" } else { "未通过" }
    );
    println!(
        "阶段 2 对的 PIN 配对成功 : {}",
        if phase2_ok { "通过" } else { "未通过" }
    );
    println!(
        "阶段 3 已配对免配对     : {}",
        if phase3_ok { "通过" } else { "未通过" }
    );
    let ok = phase1_ok && phase2_ok && phase3_ok;
    if ok {
        println!("RESULT: PASS —— PIN 真的过了 SPAKE2 密码学校验,配对真的写进了信任库");
        Ok(())
    } else {
        Err(
            format!("RESULT: FAIL —— 阶段 1 {phase1_ok} / 阶段 2 {phase2_ok} / 阶段 3 {phase3_ok}")
                .into(),
        )
    }
}

/// 主机接受循环：每次接受连接都起一个主机会话并推进它。
async fn host_accept_loop(
    listener: Arc<HostListener>,
    gate: Arc<HostTrustGate>,
    trust: Arc<Mutex<TrustStore>>,
    host_fp: beam_pair::Fingerprint,
) {
    loop {
        match listener.accept().await {
            Ok(conn) => {
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
                println!("      [主机] 接受连接,要求配对 = {require_pairing}");
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

/// 客户端发起一次连接（`pin` 为空串 = 不配对）。
async fn client_attempt(
    addr: SocketAddr,
    pin: &str,
    identity: &Identity,
    trust: &TrustStore,
) -> Result<Attempt, Box<dyn std::error::Error + Send + Sync>> {
    let net_cfg = beam_net::NetConfig {
        identity: Some(beam_net::TlsIdentity {
            cert_chain: identity.certificate_chain(),
            key: identity.private_key(),
        }),
        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..beam_net::NetConfig::default()
    };
    let conn = beam_net::NetEndpoint::connect(addr, net_cfg, Arc::new(trust.clone())).await?;
    // §5：控制双向流 + 输入单向流。
    let transport = beam_session::NetTransport::connect(conn).await?;
    let factory = ReadyTransportFactory::new(Box::new(transport) as Box<dyn Transport>);

    let mut cfg = ClientConfig::from_resolved(&beam_config::ResolvedConfig::default());
    // §3.3：客户端输入的 PIN + 本机指纹（进挑战载荷）。
    cfg.pin = if pin.is_empty() {
        None
    } else {
        Some(pin.to_string())
    };
    cfg.fingerprint = Some(identity.fingerprint());

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

    let deadline = Instant::now() + PHASE_TIMEOUT;
    let mut state = handle.state();
    let mut error = None;
    while Instant::now() < deadline {
        let _ = handle.pump();
        state = handle.state();
        if state == SessionState::Streaming || state == SessionState::Error {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // 收尾：把会话事件里的错误取出来（如实报告）。
    let mut events = handle.events();
    use futures::{FutureExt, StreamExt};
    while let Some(Some(event)) = events.next().now_or_never() {
        if let beam_session::SessionEvent::Error { code, .. } = event {
            error = Some(format!("E{code}"));
        }
    }
    Ok(Attempt {
        streaming: state == SessionState::Streaming,
        state,
        error,
    })
}
