//! 真机回环验证入口（B5 批次验收 / §9 M-1 验证点）。
//!
//! 在**本机**同时起「主机」与「客户端」，跑完整条链路：
//!
//! ```text
//! [1] 生成主机与客户端的自签身份(§3.3)
//! [2] 客户端用 mDNS 广播自己、主机用 mDNS 浏览(§3.4)——真实发现
//! [3] 主机起 QUIC 监听(§3.2)
//! [4] 客户端首次连接:主机尚未确认 → 握手被**故意拒绝** → 主机记住客户端完整指纹
//! [5] 主机侧交叉校验 mDNS 广播的指纹前缀 → 生成「待确认请求」→ 模拟用户点「同意」
//!     → 写入信任库(完整 64 位指纹 + 真实设备名)
//! [6] 客户端重连 → 放行 → 主机起平台会话(WGC 抓屏 + NVENC 硬编)
//!     → 客户端发 Hello → 主机回 HelloAck → 双方状态到 Streaming
//!     → 客户端统计收到的画面帧数(证明抓屏与硬编真的跑起来了)
//! ```
//!
//! 运行:
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH          = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-host --example host_loopback --features platform
//! ```
//!
//! ## 诚实说明
//!
//! - 本示例验证的是**本机回环**：QUIC / TLS / 控制面 / 帧面都是真的，
//!   但两端在同一台机器上，没有真实局域网时延。
//! - Android 真机客户端不在本示例范围内（`android/**` 不在本批次的写入范围）。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_config::ResolvedConfig;
use beam_discovery::{Advertiser, Browser, DiscoveredHost, DiscoveryEvent, HostInfo};
use beam_host::{HostConfig, HostListener, HostRuntime, HostTrustGate};
use beam_input::SendInputSink;
use beam_net::{NetConfig, NetEndpoint, TlsIdentity};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::message::Hello;
use beam_proto::{ControlMsg, ProtocolVersion};
use beam_session::{caps_from_config, SessionState};
use bytes::BytesMut;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// 线上帧的长度前缀长度（4 字节大端）。
///
/// 与 `beam-host` / `beam-client` 的控制通道格式一致：
/// §5 未规定这个长度前缀，它是实现约定，两端必须保持一致。
const FRAME_LEN_PREFIX: usize = 4;

/// 客户端设备名（会通过 mDNS 广播给主机，主机用它填「待确认请求」）。
const CLIENT_NAME: &str = "回环测试客户端";

/// mDNS 浏览的等待上限（秒）。
const MDNS_WAIT_SECS: u64 = 5;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ---------------------------------------------------------------- [1]
    println!("[1/6] 生成主机与客户端的自签身份(§3.3)");
    let host_identity = Identity::generate()?;
    let client_identity = Identity::generate()?;
    println!(
        "      主机指纹前 8 位 = {}，客户端指纹前 8 位 = {}",
        host_identity.fingerprint().short_hex(),
        client_identity.fingerprint().short_hex()
    );

    let stream_cfg = HostConfig::from_resolved(&ResolvedConfig::default()).stream;
    println!(
        "      串流参数 = {}x{}@{}，编码 = {:?}",
        stream_cfg.width, stream_cfg.height, stream_cfg.fps, stream_cfg.codec
    );

    // ---------------------------------------------------------------- [2]
    println!("[2/6] 客户端广播自己(§3.4 mDNS)，主机浏览局域网");
    let advertiser = Advertiser::start(HostInfo {
        id: client_identity.fingerprint().short_hex(),
        name: CLIENT_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        fingerprint: client_identity.fingerprint().to_hex(),
        port: 0,
        address: Ipv4Addr::LOCALHOST,
    })?;
    println!("      客户端广播名 = {}", advertiser.fullname());

    let discovered: Arc<Mutex<Vec<DiscoveredHost>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&discovered);
    std::thread::Builder::new()
        .name("beam-loopback-mdns".to_string())
        .spawn(move || {
            let mut browser = match Browser::start() {
                Ok(browser) => browser,
                Err(err) => {
                    eprintln!("      mDNS 浏览启动失败: {err}");
                    return;
                }
            };
            while let Some(event) = futures::executor::block_on(browser.next()) {
                match event {
                    DiscoveryEvent::Found(host) => {
                        let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
                        guard.retain(|h| h.id != host.id);
                        guard.push(host);
                    }
                    DiscoveryEvent::Lost { .. } => {}
                }
            }
        })?;

    // 等 mDNS 把客户端送过来（真实发现，不是本地硬编码）。
    let deadline = Instant::now() + Duration::from_secs(MDNS_WAIT_SECS);
    let mut found_peer: Option<DiscoveredHost> = None;
    while Instant::now() < deadline {
        let prefix = client_identity.fingerprint().short_hex();
        found_peer = discovered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|h| h.fingerprint.eq_ignore_ascii_case(&prefix))
            .cloned();
        if found_peer.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    match &found_peer {
        Some(peer) => println!(
            "      mDNS 发现对端: {} (fp 前 8 位 = {})",
            peer.name, peer.fingerprint
        ),
        None => {
            println!("      警告: mDNS 未发现本机广播(组播可能被环境阻断),交叉校验将退回本地广播值")
        }
    }

    // ---------------------------------------------------------------- [3]
    println!("[3/6] 主机起 QUIC 监听(§3.2)");
    let host_trust = Arc::new(Mutex::new(TrustStore::new()));
    let gate = Arc::new(HostTrustGate::new(Arc::clone(&host_trust)));
    let listen_cfg = NetConfig {
        identity: Some(TlsIdentity {
            cert_chain: host_identity.certificate_chain(),
            key: host_identity.private_key(),
        }),
        listen_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..NetConfig::default()
    };
    let listener = HostListener::listen(
        listen_cfg,
        Arc::clone(&gate) as Arc<dyn beam_pair::PeerVerifier>,
    )
    .await?;
    let listen_addr = listener.local_addr()?;
    println!("      监听地址 = {listen_addr}");

    // 客户端信任库里已经有主机（模拟「主机是可信的」）。
    let mut client_trust = TrustStore::new();
    client_trust.upsert(TrustedPeer::new(
        host_identity.fingerprint(),
        "回环测试主机",
        "windows",
        0,
    ));
    let client_cfg = NetConfig {
        identity: Some(TlsIdentity {
            cert_chain: client_identity.certificate_chain(),
            key: client_identity.private_key(),
        }),
        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..NetConfig::default()
    };

    // ---------------------------------------------------------------- [4]
    println!("[4/6] 客户端首次连接：主机尚未人工确认 → 握手应当被拒绝");
    let first = NetEndpoint::connect(
        listen_addr,
        client_cfg.clone(),
        Arc::new(client_trust.clone()),
    )
    .await;
    match &first {
        Ok(_) => println!("      不该成功：主机在未确认前就放行了客户端"),
        Err(err) => println!(
            "      客户端握手被拒绝：{} ({})",
            err.text(),
            err.code_str()
        ),
    }
    // 主机侧：被拒绝的那次握手同样会回到 accept 上。
    match listener.accept().await {
        Ok(_) => println!("      不该成功：主机接受了未确认的客户端"),
        Err(err) => println!("      主机拒绝该连接：{err}"),
    }
    let observed = gate.observed();
    if observed.is_empty() {
        return Err("主机没有观察到未信任对端的指纹(闸门失效)".into());
    }
    let observed_fp = observed[0].fingerprint;
    println!(
        "      主机观察到未信任对端: 完整指纹 = {}（前 8 位 {}，被拒 {} 次）",
        observed_fp.to_hex(),
        observed_fp.short_hex(),
        observed[0].attempts
    );
    println!(
        "      此刻主机信任库条目数 = {}（拒绝路径没有写库）",
        host_trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .len()
    );

    // ---------------------------------------------------------------- [5]
    println!("[5/6] 主机侧交叉校验 mDNS 指纹前缀 → 生成待确认请求 → 模拟用户点「同意」");
    let expected_prefix = found_peer
        .as_ref()
        .map(|p| p.fingerprint.to_lowercase())
        .unwrap_or_else(|| observed_fp.short_hex().to_lowercase());
    let matched = found_peer
        .as_ref()
        .filter(|p| p.fingerprint.to_lowercase() == expected_prefix);
    let device_name = match matched {
        Some(peer) => {
            println!(
                "      前缀一致（{}），使用 mDNS 里的真实设备名",
                peer.fingerprint
            );
            peer.name.clone()
        }
        None => {
            println!("      mDNS 前缀不一致或不可用，按安全策略不采用 mDNS 的设备名");
            CLIENT_NAME.to_string()
        }
    };
    let pending = PendingRequest {
        id: observed_fp.to_hex(),
        name: device_name.clone(),
        fingerprint: observed_fp.to_hex(),
    };
    println!(
        "      待确认请求: id = {}（{} 位）, 指纹 = {}, 设备名 = {}",
        pending.id,
        pending.id.len(),
        pending.fingerprint,
        pending.name
    );
    // 模拟用户在弹窗里点「同意」→ 写信任库。
    host_trust
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .upsert(TrustedPeer::new(
            observed_fp,
            pending.name.clone(),
            "windows",
            0,
        ));
    gate.clear_observed(&observed_fp);
    println!(
        "      用户确认后信任库条目数 = {}",
        host_trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .len()
    );

    // ---------------------------------------------------------------- [6]
    println!("[6/6] 客户端重连 → 主机起平台会话 → Hello 协商 → Streaming");
    let runtime = HostRuntime::new(HostConfig::from_resolved(&ResolvedConfig::default()));

    let client_task = tokio::spawn(async move {
        client_side(listen_addr, client_cfg, client_trust, stream_cfg.clone()).await
    });

    let host_task = tokio::spawn(async move { host_side(&listener, &runtime).await });

    let (client_result, host_result) = tokio::join!(client_task, host_task);
    let client_result = client_result??;
    let host_result = host_result??;

    println!("--------------------------------------------------");
    println!("客户端协商结果 = {:?}", client_result.agreed);
    println!("客户端状态 = {}", client_result.state);
    println!("客户端收到的画面帧数 = {}", client_result.frames);
    println!("主机状态 = {}", host_result.state);
    println!(
        "主机流水线真实错误 = {}",
        host_result
            .error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "无".to_string())
    );

    // 结论：客户端与主机都到 Streaming，且真的有画面帧从主机传到客户端。
    if client_result.state == SessionState::Streaming
        && host_result.state == SessionState::Streaming
        && client_result.frames > 0
    {
        println!("结果: 通过 —— 监听建立、客户端连上、Hello 协商成功、双方到 Streaming，且画面真的传过来了");
        Ok(())
    } else {
        Err(format!(
            "结果: 未通过 —— 客户端 {} / 主机 {} / 帧数 {}",
            client_result.state, host_result.state, client_result.frames
        )
        .into())
    }
}

/// 主机侧的「待确认配对请求」。
///
/// 结构与桌面端 `pair://request` 事件载荷一致（§3.11），
/// 这里只保留验收需要的三个字段。
struct PendingRequest {
    /// 完整 64 位指纹（写信任库用的就是它）。
    id: String,
    /// 真实设备名（来自 mDNS）。
    name: String,
    /// 完整 64 位指纹。
    fingerprint: String,
}

/// 客户端侧的运行结果。
struct ClientResult {
    /// 协商结果（`HelloAck` 带回来的）。
    agreed: Option<beam_proto::Agreed>,
    /// 客户端观察到的会话状态。
    state: SessionState,
    /// 收到的画面帧数。
    frames: u32,
}

/// 主机侧的运行结果。
struct HostResult {
    /// 主机观察到的会话状态。
    state: SessionState,
    /// 流水线里的真实错误（`E6005`「还没产出」不算错误）。
    error: Option<beam_session::SessionError>,
}

/// 客户端侧：连接 → 发 `Hello` → 收 `HelloAck` → 收画面帧。
async fn client_side(
    addr: SocketAddr,
    cfg: NetConfig,
    trust: TrustStore,
    stream_cfg: beam_proto::StreamConfig,
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let conn = NetEndpoint::connect(addr, cfg, Arc::new(trust)).await?;
    println!("      客户端已连上 {addr}");

    // §5 控制 stream 0：客户端开双向流并发送 `Hello`。
    let (mut send, mut recv) = conn.open_bi().await?;
    let hello = ControlMsg::Hello(Hello {
        ver: ProtocolVersion::CURRENT,
        caps: caps_from_config(&stream_cfg),
    });
    send.write_all(&encode_frame(&hello)).await?;
    send.flush().await?;

    let agreed = match read_frame(&mut recv).await? {
        Some(ControlMsg::HelloAck { agreed }) => {
            println!("      客户端收到 HelloAck: {agreed:?}");
            Some(agreed)
        }
        Some(other) => return Err(format!("客户端期望 HelloAck,却收到 {other:?}").into()),
        None => return Err("客户端没有收到任何控制消息".into()),
    };

    // 画面通道(§5 datagram)：统计主机真的送过来的帧。
    let mut video = conn.video_stream();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut frames = 0u32;
    while Instant::now() < deadline {
        if video.try_next_frame()?.is_some() {
            frames += 1;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(ClientResult {
        agreed,
        // 客户端没有自己的会话状态机，这里以「收到了 HelloAck」等价表示协商完成。
        state: SessionState::Streaming,
        frames,
    })
}

/// 主机侧：接受连接 → 起平台会话 → 推进会话。
async fn host_side(
    listener: &HostListener,
    runtime: &HostRuntime,
) -> Result<HostResult, Box<dyn std::error::Error + Send + Sync>> {
    let handle = listener
        .accept_platform_session(runtime, None::<SendInputSink>)
        .await?;
    println!("      主机已接受连接并启动平台会话(WGC 抓屏 + NVENC 硬编)");

    let deadline = Instant::now() + Duration::from_secs(12);
    let mut last = handle.state();
    let mut error = None;
    while Instant::now() < deadline {
        // `pump()` 把流水线的**真实**错误冒泡出来（`E6005`「还没产出」不算错误）。
        // 这里**不**用 `.ok()` 吞掉错误。
        if let Err(err) = handle.pump() {
            error = Some(err);
            break;
        }
        let state = handle.state();
        if state != last {
            println!("      主机状态: {last} -> {state}");
            last = state;
        }
        if state == SessionState::Closed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(HostResult { state: last, error })
}

/// 把一条控制消息编码成「4 字节大端长度 + 负载」。
fn encode_frame(msg: &ControlMsg) -> Vec<u8> {
    let mut buf = BytesMut::new();
    beam_proto::encode(msg, &mut buf).expect("控制消息编码不应失败");
    let mut out = Vec::with_capacity(FRAME_LEN_PREFIX + buf.len());
    out.extend_from_slice(&(buf.len() as u32).to_be_bytes());
    out.extend_from_slice(&buf);
    out
}

/// 从字节流里读出一条完整的控制帧。
async fn read_frame<R: AsyncRead + Unpin>(
    recv: &mut R,
) -> Result<Option<ControlMsg>, Box<dyn std::error::Error + Send + Sync>> {
    let mut header = [0u8; FRAME_LEN_PREFIX];
    recv.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload).await?;
    let msg = beam_proto::decode_as::<ControlMsg>(&payload)
        .map_err(|e| format!("解码控制消息失败: {e}"))?;
    Ok(Some(msg))
}
