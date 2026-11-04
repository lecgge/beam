//! 真机整链验证：抓屏 → 硬编 → QUIC → 硬解 → **画面上屏** + **音频进整链**。
//!
//! ## 与 `host_loopback` 的差别
//!
//! | | `host_loopback` | 本入口 |
//! |---|---|---|
//! | 主机侧 | 真抓屏（WGC）+ 真硬编（NVENC） | 同左 |
//! | 传输 | 真 QUIC | 同左 |
//! | 客户端侧 | **只数帧**（不解码、不上屏） | **真硬解 + 真上屏**（独立原生窗口） |
//! | 音频 | 不接 | **真采集 → 真编码 → 真传输 → 真分流 → 真解码 → 真回放** |
//! | 验收依据 | 收到的帧数 > 0 | `PresentInfo::textured_presents` > 0（画面真的搬进了交换链）**且**音频帧真的走完整链 |
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
//! ## 音频那一段（§3.7）的三个坑
//!
//! 1. **WGC 是内容驱动的**：屏幕静止则主机一帧都发不出（见下）。
//!    音频同理：**loopback 只采渲染流**，系统没在播声音就一帧都没有。
//!    所以本入口会自己播一段 440 Hz 测试音，让采集**确定地**有内容。
//! 2. **客户端必须用分流读点**：视频与音频走**同一条** datagram 通道，
//!    而 `beam-net` 的画面重组器**不看帧头 `type`** —— 音频数据报会被当成画面
//!    分片组装出来（负载是裸 Opus 字节，解码出来是垃圾）。
//!    本入口用 [`beam_net::DemuxStream`]：这是全仓**唯一**的分流读点。
//! 3. **同一台机器上跑两端会形成反馈回路**：客户端把收到的音频回放到默认
//!    渲染端点 ⇒ 主机端的 loopback 又把它采回来 ⇒ 无限回环。
//!    所以顺序是：**主机先停掉采集 → 客户端再回放**（[`beam_host::HostSessionHandle::stop_audio`]）。
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
//! 窗口会**真的弹出来**（本机有交互式桌面时），
//! 也会**真的播一段测试音**（主机侧一次、客户端回放一次）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_audio::{
    AudioConfig, AudioDatagramSink, AudioError, AudioMode, AudioOutput, OpusDecoderImpl, PcmFrame,
    WasapiAudioOutput,
};
use beam_decode::{D3D11VaDecoder, DecodeConfig, VideoDecoder};
use beam_host::{HostConfig, HostListener, HostRuntime, HostTrustGate};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::message::Hello;
use beam_proto::{ControlMsg, ProtocolVersion, StreamConfig};
use beam_render::{D3D11Renderer, RenderConfig, Renderer, VideoWindow, WindowHandle};
use bytes::{Bytes, BytesMut};
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
const CLIENT_RUN_SECS: u64 = 16;
/// 主机侧的时限：比客户端短 6 秒。
const HOST_RUN_SECS: u64 = CLIENT_RUN_SECS - 6;
/// 主机在会话开始后多久**停掉音频采集**。
///
/// 停掉之后客户端才回放 —— 同一台机器上跑两端时，
/// 客户端的回放会被主机的 loopback 采回来，形成反馈回路。
const HOST_STOP_AUDIO_AFTER: Duration = Duration::from_secs(4);
/// 测试音：频率（Hz）、时长（秒）、幅度（0.0–1.0）。
const TONE_HZ: f32 = 440.0;
const TONE_SECS: u64 = 3;
const TONE_AMPLITUDE: f32 = 0.2;
/// §3.7：一帧 Opus = 5 ms @ 48 kHz = 240 采样（每声道）。
const TONE_SAMPLES_PER_FRAME: usize = 240;
/// §3.7 的采样率与声道数。
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("=== Beam 真机整链验证：抓屏 → NVENC → QUIC → 硬解 → 上屏 + 音频整链 ===");

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
    println!("[3/4] 客户端建窗口 + 交换链 + 硬解上下文 + 音频输出端点");
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

    // 客户端的音频输出端点（§3.7：Opus 解码 → WASAPI 输出）。
    //
    // **回放开关**默认关：主机还在采集时，客户端的回放会被主机采回去
    // （反馈回路）。主机停掉采集之后才打开。
    let audio_cfg = AudioConfig {
        mode: AudioMode::HostAndStream,
        ..AudioConfig::default()
    };
    let client_output: SharedOutput =
        Arc::new(Mutex::new(Some(WasapiAudioOutput::new(audio_cfg.clone())?)));
    println!("      WASAPI 输出端点: 打开成功");

    // ---------------------------------------------------------------- 主机会话
    println!("[4/4] 客户端重连 → 主机起平台会话 → 协商 → 客户端硬解 + 上屏");
    let runtime = HostRuntime::new(HostConfig::from_resolved(&resolved_config()));
    println!(
        "      主机音频模式 = {:?}（§3.7）",
        runtime.config().audio.mode
    );

    let playback = Arc::new(AtomicBool::new(false));
    let client_task = tokio::spawn(async move {
        client_side(
            listen_addr,
            client_cfg,
            client_trust,
            stream_config(),
            &mut decoder,
            &mut renderer,
            audio_cfg,
            client_output,
            Arc::clone(&playback),
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

    // ---------------------------------------------------------------- 音频
    println!("--------------------------------------------------");
    println!("音频整链（§3.7）:");
    println!(
        "  主机采集并发出的音频帧数 = {}（错误 {} 次，最近码位 {:?}）",
        host_result.audio_frames_sent, host_result.audio_errors, host_result.audio_last_error
    );
    println!(
        "  客户端解码出的音频帧数 = {}（错误 {} 次）",
        client_result.audio_frames_decoded, client_result.audio_errors
    );
    println!(
        "  客户端回放到设备的帧数 = {}",
        client_result.audio_frames_played
    );
    println!(
        "  解码 PCM: {} 采样，RMS = {:.6}，峰值 = {:.6}",
        client_result.audio_samples, client_result.audio_rms, client_result.audio_peak
    );
    let audio_ok = host_result.audio_frames_sent > 0
        && client_result.audio_frames_decoded > 0
        && client_result.audio_rms > 0.0;
    println!(
        "  音频判定: {}",
        if audio_ok {
            "通过 —— 采集 → Opus 编码 → QUIC → 分流 → 解码 → 回放 全部走通"
        } else {
            "未通过 —— 音频没有完整走完整链（见上面的数字）"
        }
    );

    // §3.13：把各阶段耗时真的采下来，对照 §3.10 的预算。
    #[cfg(feature = "perf-stats")]
    {
        let snapshot = beam_perf::Perf::snapshot();
        println!("--------------------------------------------------");
        println!("各阶段耗时（§3.13 采集器 / §3.10 预算）:");
        for (stage, stat) in &snapshot.stages {
            println!(
                "  {:<14} 样本 {:>5}  均值 {:>7.3} ms  p50 {:>7.3} ms  p95 {:>7.3} ms  p99 {:>7.3} ms  最大 {:>7.3} ms",
                format!("{stage:?}"),
                stat.count,
                stat.mean_ns as f64 / 1e6,
                stat.p50_ns as f64 / 1e6,
                stat.p95_ns as f64 / 1e6,
                stat.p99_ns as f64 / 1e6,
                stat.max_ns as f64 / 1e6,
            );
        }        // §3.10 的渲染预算：解码 T1 ≤8 ms；上屏等待平均半帧（60 fps ⇒ 8.33 ms）。
        let half_frame_ms = 1000.0 / 60.0 / 2.0;
        for (stage, stat) in &snapshot.stages {
            let budget = match stage {
                beam_perf::Stage::Decode => Some(8.0),
                beam_perf::Stage::Render => Some(half_frame_ms),
                _ => None,
            };
            if let Some(budget) = budget {
                let mean_ms = stat.mean_ns as f64 / 1e6;
                let verdict = if mean_ms <= budget {
                    "达标"
                } else {
                    "超标"
                };
                println!(
                    "  预算核对 {stage:?}: 均值 {mean_ms:.3} ms vs 预算 {budget:.3} ms ⇒ {verdict}"
                );
            }
        }
    }

    let ok = client_result.state == beam_session::SessionState::Streaming
        && host_result.state == beam_session::SessionState::Streaming
        && client_result.frames_received > 0
        && client_result.textured_presents > 0
        && audio_ok;
    if ok {
        println!(
            "结果: 通过 —— 抓屏、硬编、传输、硬解、**画面真的上屏了**，**音频也真的走了整链**"
        );
        Ok(())
    } else {
        Err(format!(
            "结果: 未通过 —— 客户端 {} / 主机 {} / 收帧 {} / 上屏 {} / 音频 {}",
            client_result.state,
            host_result.state,
            client_result.frames_received,
            client_result.textured_presents,
            if audio_ok { "通过" } else { "未通过" }
        )
        .into())
    }
}

/// 主机会话配置（§3.14 默认值 + **音频打开**）。
///
/// `audio_mode = host+stream`：主机采集系统声音并串流（§3.7）。
/// 默认配置里音频是**关闭**的（`audio_mode=off`），这里显式打开 ——
/// 否则整链验证根本不会碰音频。
fn resolved_config() -> beam_config::ResolvedConfig {
    let mut cfg = beam_config::ResolvedConfig::default();
    cfg.audio.mode = beam_config::AudioMode::HostAndStream;
    cfg
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

/// 客户端侧音频的**共享输出端点**（`Arc<Mutex<Option<WasapiAudioOutput>>>`）。
///
/// 为什么要共享：接收端（[`beam_audio::AudioReceiver`]）持有输出设备，
/// 而「把缓冲里的音频回放出去」这一步要由客户端主循环发起 ——
/// 两边都要碰到同一个设备句柄。
type SharedOutput = Arc<Mutex<Option<WasapiAudioOutput>>>;

/// 客户端侧音频的**共享状态**（测量结果 + 回放缓冲）。
#[derive(Default)]
struct AudioState {
    /// 已解码的 PCM 采样数（每声道一个采样算一次）。
    samples: u64,
    /// 采样平方和（用来算 RMS）。
    sum_squares: f64,
    /// 绝对值峰值。
    peak: f32,
    /// 已回放到设备的帧数。
    played: u64,
}

/// 客户端侧的音频输出：先**测量**，再按开关决定「回放」还是「缓冲」。
///
/// - 回放开关**关**（主机还在采集时）：只测量 + 把 PCM 存起来；
/// - 回放开关**开**（主机已停采集后）：把 PCM 真的写进 WASAPI 输出设备。
///
/// 这样既验证了「音频真的走完整链」，又不会在同一台机器上形成反馈回路。
struct MeasuredOutput {
    inner: SharedOutput,
    playback: Arc<AtomicBool>,
    state: Arc<Mutex<AudioState>>,
    /// 回放开关还没打开时，PCM 暂存这里。
    buffered: Buffered,
}

impl std::fmt::Debug for MeasuredOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeasuredOutput")
            .field("playback", &self.playback.load(Ordering::Relaxed))
            .finish()
    }
}

impl AudioOutput for MeasuredOutput {
    fn play(&mut self, pcm: &PcmFrame) -> Result<(), AudioError> {
        // 测量（交错 i16 ⇒ 每 2 字节一个采样）。
        let mut sum_squares = 0f64;
        let mut peak = 0f32;
        let mut count = 0u64;
        for chunk in pcm.samples.chunks_exact(2) {
            let v = i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0;
            sum_squares += f64::from(v) * f64::from(v);
            peak = peak.max(v.abs());
            count += 1;
        }
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.samples += count;
            state.sum_squares += sum_squares;
            state.peak = state.peak.max(peak);
        }

        if self.playback.load(Ordering::Relaxed) {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(output) = guard.as_mut() {
                output.play(pcm)?;
                self.state.lock().unwrap_or_else(|e| e.into_inner()).played += 1;
            }
        } else {
            // 回放开关还没打开：把 PCM 存起来，等主机停掉采集后再放。
            self.buffered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(pcm.samples.clone());
        }
        Ok(())
    }
}

/// 客户端侧音频的**回放缓冲**（PCM 帧）。
type Buffered = Arc<Mutex<Vec<Bytes>>>;

/// 把缓冲里的音频回放到输出设备（主机已停采集 ⇒ 不会形成反馈回路）。
fn flush_buffer(buffer: &Buffered, output: &SharedOutput, state: &Arc<Mutex<AudioState>>) {
    let buffered: Vec<Bytes> = {
        let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *buf)
    };
    if buffered.is_empty() {
        return;
    }
    let mut guard = output.lock().unwrap_or_else(|e| e.into_inner());
    let Some(device) = guard.as_mut() else {
        return;
    };
    for samples in buffered {
        let pcm = PcmFrame {
            samples,
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
        };
        match device.play(&pcm) {
            Ok(()) => {
                state.lock().unwrap_or_else(|e| e.into_inner()).played += 1;
            }
            Err(e) => {
                println!("      音频回放失败 {} — {e}", e.code_str());
                break;
            }
        }
    }
}

/// 主机侧结果。
struct HostResult {
    state: beam_session::SessionState,
    error: Option<beam_session::SessionError>,
    /// 已发出的音频帧数（§3.7）。
    audio_frames_sent: u64,
    /// 音频错误次数。
    audio_errors: u64,
    /// 最近一次音频错误的码位。
    audio_last_error: Option<u16>,
}

/// 客户端侧结果。
struct ClientResult {
    state: beam_session::SessionState,
    frames_received: u32,
    textured_presents: u64,
    converted_presents: u64,
    cleared_presents: u64,
    /// 已解码的音频帧数。
    audio_frames_decoded: u64,
    /// 已回放到设备的音频帧数。
    audio_frames_played: u64,
    /// 音频错误次数。
    audio_errors: u64,
    /// 解码 PCM 的采样数。
    audio_samples: u64,
    /// 解码 PCM 的 RMS。
    audio_rms: f64,
    /// 解码 PCM 的绝对值峰值。
    audio_peak: f32,
}

/// 主机侧：接受连接 → 起平台会话（真抓屏 + 真硬编 + 真音频采集）→ 推进到 Streaming。
async fn host_side(
    listener: &HostListener,
    runtime: &HostRuntime,
) -> Result<HostResult, Box<dyn std::error::Error + Send + Sync>> {
    let mut handle = listener
        .accept_platform_session(runtime, None::<beam_host::NoopInputSink>)
        .await?;
    println!("      主机已接受连接并启动平台会话（WGC 抓屏 + NVENC 硬编 + WASAPI 采集）");

    // 播一段测试音：loopback 只采「渲染流」，系统没在播声音就一帧都采不到。
    // 测试音保证采集**确定地**有内容（验证入口必须可重复）。
    let tone = TonePlayer::start();
    println!("      正在播放 {TONE_HZ} Hz 测试音（{TONE_SECS} 秒），供 loopback 采集");

    let start = Instant::now();
    let deadline = start + Duration::from_secs(HOST_RUN_SECS);
    let mut audio_stopped = false;
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
        // 到点停掉音频采集：客户端随后回放时，主机不会再把回放采回来。
        if !audio_stopped && start.elapsed() >= HOST_STOP_AUDIO_AFTER {
            let stopped = handle.stop_audio().await;
            println!(
                "      主机已停止音频采集（{}）—— 客户端回放不会再被采回来",
                if stopped {
                    "成功"
                } else {
                    "本来就没有采集任务"
                }
            );
            audio_stopped = true;
        }
        if state == beam_session::SessionState::Closed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let audio = handle.audio_stats().map(|s| s.snapshot());
    drop(tone);
    Ok(HostResult {
        state: last,
        error,
        audio_frames_sent: audio.map(|a| a.frames_sent).unwrap_or(0),
        audio_errors: audio.map(|a| a.errors).unwrap_or(0),
        audio_last_error: audio.and_then(|a| a.last_error_code),
    })
}

/// 在**独立线程**里播放测试音（WASAPI 输出会阻塞，写在 tokio 线程里不合适）。
struct TonePlayer {
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TonePlayer {
    /// 起播放线程。
    fn start() -> Self {
        let handle = std::thread::Builder::new()
            .name("beam-loopback-tone".to_string())
            .spawn(|| {
                if let Err(e) = play_tone() {
                    println!("      测试音播放失败: {e}");
                }
            })
            .expect("起测试音线程失败");
        Self {
            handle: Some(handle),
        }
    }
}

impl Drop for TonePlayer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 播放 440 Hz 测试音（正弦，双声道 i16 @ 48 kHz，每帧 5 ms）。
fn play_tone() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cfg = AudioConfig {
        mode: AudioMode::HostAndStream,
        ..AudioConfig::default()
    };
    let mut output = WasapiAudioOutput::new(cfg)?;
    let frames = (TONE_SECS * 1000) / 5; // 5 ms 一帧
    let step = 2.0 * std::f32::consts::PI * TONE_HZ / SAMPLE_RATE as f32;
    let mut phase = 0f32;
    for _ in 0..frames {
        let mut samples = Vec::with_capacity(TONE_SAMPLES_PER_FRAME * 2 * 2);
        for _ in 0..TONE_SAMPLES_PER_FRAME {
            let value = (TONE_AMPLITUDE * phase.sin() * i16::MAX as f32) as i16;
            samples.extend_from_slice(&value.to_le_bytes());
            samples.extend_from_slice(&value.to_le_bytes());
            phase += step;
        }
        output.play(&PcmFrame {
            samples: Bytes::from(samples),
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
        })?;
    }
    Ok(())
}

/// 客户端侧：连 QUIC → 发 `Hello` → 收帧（画面 + 音频分流）→ 硬解 → 上屏。
#[allow(clippy::too_many_arguments)]
async fn client_side(
    addr: SocketAddr,
    cfg: beam_net::NetConfig,
    trust: TrustStore,
    stream: StreamConfig,
    decoder: &mut D3D11VaDecoder,
    renderer: &mut D3D11Renderer,
    audio_cfg: AudioConfig,
    client_output: SharedOutput,
    playback: Arc<AtomicBool>,
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

    // 画面 + 音频**共用**同一条 datagram 通道（§5）⇒ 必须用分流读点。
    //
    // `beam_net::VideoStream` 会把到达的**所有**数据报都塞进画面重组器，
    // 而重组器**不看帧头 `type`** ⇒ 音频数据报会被当成画面分片（垃圾负载）。
    // `DemuxStream` 是全仓唯一的分流读点。
    let mut demux = beam_net::DemuxStream::new(&conn);

    // 音频接收端（§3.7）：Opus 解码 → 测量 → 回放/缓冲。
    let opus_decoder = OpusDecoderImpl::new(audio_cfg.clone())?;
    let audio_state = Arc::new(Mutex::new(AudioState::default()));
    let buffered: Buffered = Arc::new(Mutex::new(Vec::new()));
    let measured = MeasuredOutput {
        inner: Arc::clone(&client_output),
        playback: Arc::clone(&playback),
        state: Arc::clone(&audio_state),
        buffered: Arc::clone(&buffered),
    };
    let audio_stats = beam_audio::AudioReceiveStatsHandle::default();
    let mut audio_receiver =
        beam_audio::AudioReceiver::with_stats(opus_decoder, measured, audio_stats.clone());

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

        // 主机已停采集 ⇒ 现在回放缓冲里的音频（不会形成反馈回路）。
        if playback.load(Ordering::Relaxed) {
            flush_buffer(&buffered, &client_output, &audio_state);
        }

        // 读走已到达的数据报：画面进重组器，音频走音频支路。
        let drained = demux.drain(|kind, datagram| {
            if !matches!(kind, beam_proto::FrameKind::Audio) {
                return;
            }
            // 音频失败只记录、不上抛：画面照旧（§3.7 的降级语义）。
            if let Err(e) = audio_receiver.handle_datagram(datagram) {
                tracing::warn!(code = %e.code_str(), reason = %e, "音频数据报处理失败");
            }
        });
        match drained {
            Ok(Some(frame)) => {
                frames_received += 1;
                // 分流后的画面已经是**完整帧**（重组 + 抖动缓冲都做过了）。
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

    // 最后再冲一次缓冲（主机停采集的时刻可能刚好在最后一次轮询之后）。
    flush_buffer(&buffered, &client_output, &audio_state);

    let info = renderer.present_info();
    let audio_stats = audio_stats.snapshot();
    let state = audio_state.lock().unwrap_or_else(|e| e.into_inner());
    let rms = if state.samples > 0 {
        (state.sum_squares / state.samples as f64).sqrt()
    } else {
        0.0
    };
    Ok(ClientResult {
        // 本示例自己握手（不走会话层状态机）⇒ 「Streaming」= 协商成功且在收帧。
        state: beam_session::SessionState::Streaming,
        frames_received,
        textured_presents: info.textured_presents,
        converted_presents: info.converted_presents,
        cleared_presents: info.cleared_presents,
        audio_frames_decoded: audio_stats.frames_decoded,
        audio_frames_played: state.played,
        audio_errors: audio_stats.errors,
        audio_samples: state.samples,
        audio_rms: rms,
        audio_peak: state.peak,
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
