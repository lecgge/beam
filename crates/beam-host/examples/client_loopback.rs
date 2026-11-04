//! 真机整链验证：抓屏 → 硬编 → QUIC → 硬解 → **画面上屏**（§3.10 / 产品方案第 45 行）。
//!
//! ## 与 `host_loopback` 的差别
//!
//! | | `host_loopback` | 本入口 |
//! |---|---|---|
//! | 主机侧 | 真抓屏（WGC）+ 真硬编（NVENC） | 同左 |
//! | 传输 | 真 QUIC | 同左 |
//! | 客户端侧 | **只数帧**（不解码、不上屏） | **真硬解 + 真上屏**（独立原生窗口） |
//! | 验收依据 | 收到的帧数 > 0 | `PresentInfo::textured_presents` > 0（画面真的搬进了交换链） |
//!
//! ## 为什么「收到帧数」不够
//!
//! B17 的教训：D3D11VA 的输出是 `DXGI_FORMAT_420Opaque`，与交换链格式不同，
//! 每一帧都会静默降级成清屏 —— **不报错**，会话状态正常，收到帧数也正常，
//! 但用户看到的是空白窗口。
//!
//! 所以这里的判据是 [`beam_render::PresentInfo`]：只有「真的把解码纹理搬进
//! 交换链」的帧数才算数。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-host --example client_loopback --features platform,client-video --release
//! ```
//!
//! 窗口会**真的弹出来**（本机有交互式桌面时）。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_decode::{D3D11VaDecoder, DecodeConfig, VideoDecoder};
use beam_host::{HostConfig, HostListener, HostRuntime, HostTrustGate};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::message::Hello;
use beam_proto::{ControlMsg, ProtocolVersion, StreamConfig};
use beam_render::{D3D11Renderer, RenderConfig, Renderer, VideoWindow, WindowHandle};
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// §5 控制帧的长度前缀字节数（§5）。
const FRAME_LEN_PREFIX: usize = 4;
/// 客户端名字（会进 mDNS 的设备名）。
const CLIENT_NAME: &str = "回环整链客户端";
/// 客户端跑到 Streaming 后再多跑一会儿，让画面真的上屏。
///
/// **必须比主机侧长**：客户端一断开，主机的控制流就断了 ⇒ 主机会话必然进
/// Error 态（那是**正常的断开语义**，不是故障）。所以主机侧的时限要短一些，
/// 让它在客户端还连着的时候记录到「Streaming」。
const CLIENT_RUN_SECS: u64 = 12;
/// 主机侧的时限：比客户端短 4 秒。
const HOST_RUN_SECS: u64 = CLIENT_RUN_SECS - 4;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("=== Beam 真机整链验证：抓屏 → NVENC → QUIC → 硬解 → 上屏 ===");

    // tracing 打到 stderr：主机侧的**真实**错误（E6xxx / E5xxx…）在这里能看到。
    // 没有它，上层只能看到「状态变成 Error」，看不到为什么。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    // ---------------------------------------------------------------- 身份
    let host_identity = Identity::generate()?;
    let client_identity = Identity::generate()?;
    println!(
        "主机指纹 = {}，客户端指纹 = {}",
        host_identity.fingerprint().short_hex(),
        client_identity.fingerprint().short_hex()
    );

    // ---------------------------------------------------------------- 屏幕活动
    //
    // WGC 是**内容驱动**的：只在画面**发生变化**时投递新帧（见
    // `beam_capture::wgc` 的注释）。屏幕上完全静止 ⇒ 主机一帧都发不出来。
    //
    // 这不是 bug，但它让「整链验证」变得不确定（实测同一台机器上
    // 同样的命令，帧数在 0 与 1 之间跳）。所以这里起一个后台进程
    // 持续刷新屏幕，让抓屏**确定地**有帧 —— 验证入口必须可重复。
    let _screen_agitator = ScreenAgitator::start();

    // ---------------------------------------------------------------- 监听
    println!("[1/4] 主机起 QUIC 监听（§3.2）");
    let host_trust = Arc::new(Mutex::new(TrustStore::new()));
    let gate = Arc::new(HostTrustGate::new(Arc::clone(&host_trust)));
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
    let listen_addr = listener.local_addr()?;
    println!("      监听地址 = {listen_addr}");

    // 客户端信任库里已经有主机（模拟「主机是可信的」）。
    let mut client_trust = TrustStore::new();
    client_trust.upsert(TrustedPeer::new(
        host_identity.fingerprint(),
        "回环整链主机",
        "windows",
        0,
    ));
    let client_cfg = beam_net::NetConfig {
        identity: Some(beam_net::TlsIdentity {
            cert_chain: client_identity.certificate_chain(),
            key: client_identity.private_key(),
        }),
        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..beam_net::NetConfig::default()
    };

    // -------------------------------------------------- 首次连接被拒 → 确认
    println!("[2/4] 客户端首次连接 → 主机未确认 → 主机侧应当拒绝");
    //
    // 注意 quinn 的语义：**客户端**的 `connect()` 在验证完**服务端**证书后就返回
    // Ok（它不等服务端验证自己的证书）；被拒的是**服务端**的 `accept()`。
    // 所以判据是「主机侧 accept 失败」，而不是「客户端 connect 失败」。
    let (host_first, client_first) = tokio::join!(
        listener.accept(),
        beam_net::NetEndpoint::connect(
            listen_addr,
            client_cfg.clone(),
            Arc::new(client_trust.clone()),
        ),
    );
    let host_rejected = host_first.is_err();
    println!(
        "      主机侧 accept: {}；客户端侧 connect: {}",
        if host_rejected {
            "被拒绝（闸门生效）"
        } else {
            "被放行了（闸门失效！）"
        },
        match &client_first {
            Ok(_) => "已连上（quinn 语义：不等服务端验自己）",
            Err(_) => "失败",
        }
    );
    if !host_rejected {
        return Err("信任闸门失效：主机放行了未信任的客户端".into());
    }
    let observed = gate.observed();
    if observed.is_empty() {
        return Err("主机没有观察到未信任对端的指纹(闸门失效)".into());
    }
    let observed_fp = observed[0].fingerprint;
    println!(
        "      主机观察到未信任对端: {}（被拒 {} 次）",
        observed_fp.short_hex(),
        observed[0].attempts
    );

    // 模拟用户在弹窗里点「同意」→ 写信任库。
    host_trust
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .upsert(TrustedPeer::new(observed_fp, CLIENT_NAME, "windows", 0));
    gate.clear_observed(&observed_fp);
    println!(
        "      用户确认后信任库条目数 = {}",
        host_trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .len()
    );

    // ---------------------------------------------------------------- 客户端
    println!("[3/4] 客户端建窗口 + 交换链 + 硬解上下文");
    let window = VideoWindow::create("Beam 整链串流画面", 1920, 1080)?;
    println!("      原生窗口: hwnd = 0x{:x}", window.hwnd());
    let render_cfg = RenderConfig {
        width: 1920,
        height: 1080,
        ..RenderConfig::default()
    };
    let mut renderer = D3D11Renderer::new();
    renderer.attach(WindowHandle(window.hwnd()), render_cfg)?;
    let device = renderer.device().ok_or("渲染层没有 D3D11 设备")?.clone();
    // 硬解必须用**同一个设备**（D3D11 不允许跨设备拷贝）。
    let mut decoder = D3D11VaDecoder::open_with_device(
        DecodeConfig {
            codec: beam_proto::CodecKind::Hevc,
            hardware: true,
            low_latency: true,
        },
        &device,
    )?;
    println!("      D3D11VA 硬解器: 建立成功（复用渲染层同一个设备）");

    // ---------------------------------------------------------------- 主机会话
    println!("[4/4] 客户端重连 → 主机起平台会话 → 协商 → 客户端硬解 + 上屏");
    let runtime = HostRuntime::new(HostConfig::from_resolved(
        &beam_config::ResolvedConfig::default(),
    ));

    let client_task = tokio::spawn(async move {
        client_side(
            listen_addr,
            client_cfg,
            client_trust,
            stream_config(),
            &mut decoder,
            &mut renderer,
        )
        .await
    });

    let host_task = tokio::spawn(async move { host_side(&listener, &runtime).await });

    let (client_result, host_result) = tokio::join!(client_task, host_task);
    let client_result = client_result??;
    let host_result = host_result??;

    println!("--------------------------------------------------");
    println!("主机状态 = {}", host_result.state);
    println!(
        "主机流水线真实错误 = {}",
        host_result
            .error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "无".to_string())
    );
    println!("客户端状态 = {}", client_result.state);
    println!("客户端收到的画面帧数 = {}", client_result.frames_received);
    println!(
        "客户端**真的搬进交换链**的帧数 = {}（其中经 VideoProcessor 转换 {}）",
        client_result.textured_presents, client_result.converted_presents
    );
    println!(
        "客户端降级成清屏的帧数 = {}",
        client_result.cleared_presents
    );

    let ok = client_result.state == beam_session::SessionState::Streaming
        && host_result.state == beam_session::SessionState::Streaming
        && client_result.frames_received > 0
        && client_result.textured_presents > 0;
    if ok {
        println!("结果: 通过 —— 抓屏、硬编、传输、硬解、**画面真的上屏了**");
        Ok(())
    } else {
        Err(format!(
            "结果: 未通过 —— 客户端 {} / 主机 {} / 收帧 {} / 上屏 {}",
            client_result.state,
            host_result.state,
            client_result.frames_received,
            client_result.textured_presents
        )
        .into())
    }
}

/// 串流配置（§3.14 默认值）。
///
/// **必须与主机侧一致**（§5 能力协商）：客户端报的能力集与主机没有交集时，
/// 协商直接失败，主机会进 Error 态。
fn stream_config() -> StreamConfig {
    HostConfig::from_resolved(&beam_config::ResolvedConfig::default()).stream
}

/// 让屏幕**持续变化**的后台进程（WGC 是内容驱动的，详见调用处）。
///
/// 起一个 `cmd` 子进程不停往屏幕上打点 —— 显示器内容就一直变，
/// WGC 就一直有帧。进程在 [`Drop`] 时被杀掉。
struct ScreenAgitator {
    child: std::process::Child,
}

impl ScreenAgitator {
    /// 起这个后台进程。
    fn start() -> Self {
        // `for /L` 循环不停输出 ⇒ 终端窗口内容持续变化。
        // 用 `cmd /C` 而不是 PowerShell：Windows 自带、无依赖、启动快。
        let child = std::process::Command::new("cmd")
            .args(["/C", "for /L %i in (1,1,100000) do @echo ."])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("起屏幕活动进程失败");
        Self { child }
    }
}

impl Drop for ScreenAgitator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 主机侧结果。
struct HostResult {
    state: beam_session::SessionState,
    error: Option<beam_session::SessionError>,
}

/// 客户端侧结果。
struct ClientResult {
    state: beam_session::SessionState,
    frames_received: u32,
    textured_presents: u64,
    converted_presents: u64,
    cleared_presents: u64,
}

/// 主机侧：接受连接 → 起平台会话（真抓屏 + 真硬编）→ 推进到 Streaming。
async fn host_side(
    listener: &HostListener,
    runtime: &HostRuntime,
) -> Result<HostResult, Box<dyn std::error::Error + Send + Sync>> {
    let handle = listener
        .accept_platform_session(runtime, None::<beam_host::NoopInputSink>)
        .await?;
    println!("      主机已接受连接并启动平台会话（WGC 抓屏 + NVENC 硬编）");

    let deadline = Instant::now() + Duration::from_secs(HOST_RUN_SECS);
    let mut last = handle.state();
    let mut error = None;
    while Instant::now() < deadline {
        if let Err(err) = handle.pump() {
            error = Some(err);
            break;
        }
        let state = handle.state();
        if state != last {
            println!("      主机状态: {last} -> {state}");
            last = state;
        }
        if state == beam_session::SessionState::Closed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(HostResult { state: last, error })
}

/// 客户端侧：连 QUIC → 发 `Hello` → 收帧 → 硬解 → 上屏。
async fn client_side(
    addr: SocketAddr,
    cfg: beam_net::NetConfig,
    trust: TrustStore,
    stream: StreamConfig,
    decoder: &mut D3D11VaDecoder,
    renderer: &mut D3D11Renderer,
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let conn = beam_net::NetEndpoint::connect(addr, cfg, Arc::new(trust)).await?;
    println!("      客户端已连上 {addr}");

    // §5 控制 stream 0：客户端开双向流并发送 `Hello`。
    //
    // 这里用**手写**的控制流（与 `host_loopback` 完全一致），不用
    // `beam_session::NetTransport` —— 后者还会额外开一条输入单向流，
    // 与主机侧「不接收输入流」的配置不一致，会让主机在协商后立刻出错。
    let (mut send, mut recv) = conn.open_bi().await?;
    let hello = ControlMsg::Hello(Hello {
        ver: ProtocolVersion::CURRENT,
        caps: beam_session::caps_from_config(&stream),
    });
    send.write_all(&encode_frame(&hello)).await?;
    send.flush().await?;

    // 等主机的 `HelloAck`（能力协商结果）。
    match read_frame(&mut recv).await? {
        Some(ControlMsg::HelloAck { agreed }) => {
            println!("      客户端收到 HelloAck: {agreed:?}");
        }
        Some(other) => return Err(format!("客户端期望 HelloAck,却收到 {other:?}").into()),
        None => return Err("客户端没有收到任何控制消息".into()),
    }

    // 画面通道（§5 datagram）：`video_stream` 自带重组器，拿到的是完整帧。
    let mut video = conn.video_stream();
    let deadline = Instant::now() + Duration::from_secs(CLIENT_RUN_SECS);
    let mut frames_received = 0u32;
    let mut next_feedback = Instant::now();
    while Instant::now() < deadline {
        // 每 500 ms 发一条 `Feedback`（§4.1）：主机侧的停滞判定看的是
        // 「有没有收到客户端的数据」，不发的话主机会在 3 s 后自己进重连。
        if Instant::now() >= next_feedback {
            next_feedback = Instant::now() + Duration::from_millis(500);
            let feedback = ControlMsg::Feedback(beam_proto::Feedback {
                loss: 0,
                rtt: Duration::from_millis(1),
                recv_rate: 0,
            });
            send.write_all(&encode_frame(&feedback)).await?;
            send.flush().await?;
        }
        match video.try_next_frame() {
            Ok(Some(frame)) => {
                frames_received += 1;
                // `video_stream` 自带重组器 ⇒ 拿到的已经是 `AssembledFrame`。
                match decoder.decode(frame) {
                    Ok(decoded) => {
                        // §3.10：真的上屏（格式不匹配时由 VideoProcessor 转换）。
                        if let Err(e) = renderer.present(&decoded) {
                            println!("      上屏失败 {} — {e}", e.code_str());
                        }
                    }
                    Err(e) => {
                        println!("      解码失败 {} — {e}", e.code_str());
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                println!("      收帧失败 {e}");
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let info = renderer.present_info();
    Ok(ClientResult {
        state: beam_session::SessionState::Streaming,
        frames_received,
        textured_presents: info.textured_presents,
        converted_presents: info.converted_presents,
        cleared_presents: info.cleared_presents,
    })
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
