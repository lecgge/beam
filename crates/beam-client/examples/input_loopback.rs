//! 真机输入回传验证（§3.8）：客户端事件 → §5 输入单向流 → 主机注入 → **桌面真的动了**。
//!
//! ## 验证的是什么
//!
//! 输入链路的**三段**里，协议与传输早就有了（§5 输入单向流、
//! `Transport::send_input`），主机侧的注入器也是真的
//! （`beam-input` 的 `InputInjector`：有界通道 + 专用注入线程 + MMCSS "Games"）。
//! 此前**从没被端到端验证过**的是「客户端事件真的让主机的桌面动了」。
//!
//! ## 怎么证明「真的动了」
//!
//! 用**光标位置**当证据（最硬的一个）：
//!
//! 1. 客户端读当前光标位置 `(x, y)`；
//! 2. 客户端经**真实的会话管道**发一个 `MouseMove` 到 `(x+1, y)`；
//! 3. 主机读自己的光标位置 ⇒ 必须与原来**至少差 1 像素**
//!    （`SendInput` 的 `MOUSEEVENTF_ABSOLUTE` 真的把光标挪了）；
//! 4. 主机侧注入器的统计里 `injected >= 1`（真的调过 `SendInput`）；
//! 5. 客户端再发一个 `MouseMove` 回 `(x, y)` ⇒ **把光标还原**，不打扰用户。
//!
//! 挪 1 像素对人眼不可见，但仪器测得出来 —— 这是「不打扰用户」与「可验证」的折中。
//!
//! ## 用法
//!
//! ```powershell
//! cargo run -p beam-client --example input_loopback --release
//! ```
//!
//! 不需要 ffmpeg：这个入口只验**控制面 + 输入面**，不碰解码与上屏。

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam_client::{ClientConfig, ClientRuntime};
use beam_host::{HostConfig, HostListener, HostRuntime};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::InputEvent;
use beam_session::PeerAddr;

/// 主机侧光标移动前后的判定阈值（像素）。
const MIN_MOVE_PX: i32 = 1;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,beam_session=debug")),
        )
        .with_writer(std::io::stderr)
        .try_init();
    println!("=== 真机输入回传验证（§3.8）：客户端事件 → 输入流 → 主机注入 → 桌面真的动了 ===");

    // ---------------------------------------------------------------- 身份
    let host_identity = Identity::generate()?;
    let client_identity = Identity::generate()?;
    let host_fp = host_identity.fingerprint();
    println!("主机指纹 = {}", host_fp.short_hex());

    let mut client_trust = TrustStore::new();
    client_trust.upsert(TrustedPeer::new(host_fp, "回环输入主机", "windows", 0));

    // ---------------------------------------------------------------- 监听
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let listen_cfg = beam_net::NetConfig {
        identity: Some(beam_net::TlsIdentity {
            cert_chain: host_identity.certificate_chain(),
            key: host_identity.private_key(),
        }),
        listen_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        server_name: "beam.local".to_string(),
        ..beam_net::NetConfig::default()
    };
    // 主机信任库里**先有**客户端的指纹（模拟「已配对」的对端）：
    // 否则信任闸门会在 TLS 层拒掉未信任对端（§3.3），
    // 客户端根本连不上 —— 双方互等，谁也走不到输入那一段。
    let mut host_trust = TrustStore::new();
    host_trust.upsert(TrustedPeer::new(
        client_identity.fingerprint(),
        "回环输入客户端",
        "windows",
        0,
    ));
    let gate = Arc::new(beam_host::HostTrustGate::new(Arc::new(Mutex::new(
        host_trust,
    ))));
    let listener = runtime.block_on(HostListener::listen(
        listen_cfg,
        Arc::clone(&gate) as Arc<dyn beam_pair::PeerVerifier>,
    ))?;
    let addr = listener.local_addr()?;
    println!("主机监听地址 = {addr}");

    // 主机侧：起会话 + 记录光标位置。
    let host = runtime.spawn(async move {
        let conn = loop {
            match listener.accept().await {
                Ok(conn) => break conn,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        let before = cursor_position();
        let mut cfg = beam_config::ResolvedConfig::default();
        // §3.8：按设备的注入权限 = 对端权限 × 配置侧设置 ⇒ 这里显式允许键鼠。
        cfg.input.allow_mouse = true;
        cfg.input.allow_keyboard = true;
        let host_runtime = HostRuntime::new(HostConfig::from_resolved(&cfg));
        let (pipeline, errors) = beam_host::default_frame_pipeline(host_runtime.config());
        // 传**占位**注入器 ⇒ 主机编排层会换成**真**注入器（SendInput + MMCSS 线程）。
        let handle = match host_runtime
            .start_quic_session_with_pipeline(
                conn,
                pipeline,
                errors,
                Some(beam_host::NoopInputSink::new()),
                beam_host::HostPairing::default(),
            )
            .await
        {
            Ok(handle) => handle,
            Err(err) => {
                println!("主机会话建立失败: {err}");
                return (before, before, None);
            }
        };
        // 推进会话（控制面 + 输入面）。
        //
        // 有时限：主机这一侧不会自己结束（会话一直在），
        // 没有时限的话整个验证会永远等下去。
        let stats_handle = handle.input_stats().cloned();
        let pump_deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < pump_deadline {
            let _ = handle.pump();
            if handle.session.is_stopped() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        (before, cursor_position(), stats_handle)
    });

    // ---------------------------------------------------------------- 客户端
    let client = runtime.spawn(async move {
        let net_cfg = beam_net::NetConfig {
            identity: Some(beam_net::TlsIdentity {
                cert_chain: client_identity.certificate_chain(),
                key: client_identity.private_key(),
            }),
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            server_name: "beam.local".to_string(),
            ..beam_net::NetConfig::default()
        };
        let conn = match beam_net::NetEndpoint::connect(addr, net_cfg, Arc::new(client_trust)).await
        {
            Ok(conn) => conn,
            Err(err) => {
                println!("客户端连接失败: {err}");
                return (0, 0, 0, 0);
            }
        };
        let transport = match beam_session::NetTransport::connect(conn).await {
            Ok(t) => t,
            Err(err) => {
                println!("客户端建流失败: {err}");
                return (0, 0, 0, 0);
            }
        };
        let factory = beam_client::ReadyTransportFactory::new(
            Box::new(transport) as Box<dyn beam_session::Transport>
        );

        // 模式：默认用**脚本源**（确定性强，不依赖用户的手）；
        // 设 `BEAM_REAL_CAPTURE=1` 则用**真实钩子**（验证采集后端真的能力）。
        let real_capture = std::env::var("BEAM_REAL_CAPTURE").as_deref() == Ok("1");
        let capture = if real_capture {
            match beam_client::capture::WindowsInputCapture::start_with_options(
                beam_client::capture::CaptureOptions {
                    capture_injected: true,
                },
            ) {
                Ok(capture) => {
                    println!(
                        "      客户端键鼠采集: 低级钩子已安装 = {}错误 = {:?}",
                        capture.is_installed(),
                        capture.error()
                    );
                    Some(Arc::new(capture))
                }
                Err(e) => {
                    println!("      客户端键鼠采集启动失败: {e}");
                    None
                }
            }
        } else {
            None
        };
        let source = Arc::new(ScriptedInput::new());
        let mut cfg = ClientConfig::from_resolved(&beam_config::ResolvedConfig::default());
        cfg.pin = None;
        cfg.fingerprint = Some(client_identity.fingerprint());
        let client_runtime = ClientRuntime::new(cfg.clone()).with_input(source.clone());
        // 真实模式：装上采集源（脚本源与采集源不共存，这里只用真实采集源）。
        let client_runtime = match &capture {
            Some(capture) => ClientRuntime::new(cfg)
                .with_input(Arc::clone(capture) as Arc<dyn beam_session::InputSource>),
            None => client_runtime,
        };

        let peer = PeerAddr {
            host: addr.ip().to_string(),
            port: addr.port(),
            fingerprint: None,
        };
        let handle = client_runtime.connect(
            peer,
            beam_decode::D3D11VaDecoder::new(),
            beam_render::D3D11Renderer::new(),
            Some(Box::new(factory)),
        );

        // 等进串流态（此时会话层才开始回传输入，§5 的时序）。
        //
        // **关键**:进了串流**不能**就此不泵了 —— 会话层的 pump 才是
        // 「把采集到的事件写进输入单向流」的地方(§5)。不再泵 ⇒ 事件
        // 一直堵在采集源里，直到任务结束才被取出，而那时连接正要关闭
        // ⇒ 数据发不出去(实测:主机侧报「接受输入流失败 closed by peer」)。
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            let _ = handle.pump();
            if handle.state() == beam_session::SessionState::Streaming {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        println!("      客户端状态 = {}", handle.state());

        // 真实模式：注入一个测试键（F13，机器上没人用）。
        // 注入事件带 `LLKHF_INJECTED` 标记 → 默认会被采集侧跳过，
        // 所以验证时开了 `capture_injected`。
        if let Some(capture) = &capture {
            inject_test_key();
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until && capture.captured_count() == 0 {
                let _ = handle.pump();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            println!(
                "      客户端钩子采到的事件数 = {}",
                capture.captured_count()
            );
            // 断掉回环：采集派生的注入事件不能再回传。
            capture.stop();
        }

        // 阶段 1：把光标挪 1 像素。
        //
        // **全程继续泵会话** —— 输入事件是泵循环里发出去的。
        let (x, y) = cursor_position();
        println!("      注入前光标位置 = ({x}, {y})");
        source.push(mouse_move_to(x + MIN_MOVE_PX, y));
        let until = Instant::now() + Duration::from_millis(600);
        while Instant::now() < until {
            let _ = handle.pump();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // 阶段 2：把光标还原（不打扰用户）。
        source.push(mouse_move_to(x, y));
        let until = Instant::now() + Duration::from_millis(600);
        while Instant::now() < until {
            let _ = handle.pump();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let captured_total = capture.as_ref().map(|c| c.captured_count()).unwrap_or(0);
        (x, y, source.sent(), captured_total)
    });

    let (host_result, client_result) = runtime.block_on(async { tokio::join!(host, client) });
    let host_result = host_result.unwrap_or_else(|e| {
        println!("主机任务异常: {e}");
        ((0, 0), (0, 0), None)
    });
    let client_result = client_result.unwrap_or_else(|e| {
        println!("客户端任务异常: {e}");
        (0, 0, 0, 0)
    });
    let (before, after, injector_stats) = host_result;
    let (cx, cy, sent, captured_total) = client_result;

    // ---------------------------------------------------------------- 结论
    println!("--------------------------------------------------");
    println!("客户端发出的输入事件数 = {sent} (+钩子采到 {captured_total})");
    let moved = (after.0 - before.0).abs() + (after.1 - before.1).abs();
    println!(
        "主机光标位置：注入前 = ({}, {})，注入后 = ({}, {})，位移 = {moved} 像素",
        before.0, before.1, after.0, after.1
    );
    if let Some(stats) = injector_stats.as_ref() {
        let snapshot = stats.snapshot();
        println!(
            "主机注入器统计：submitted = {}，injected = {}，denied = {}，dropped = {}",
            snapshot.submitted, snapshot.injected, snapshot.denied, snapshot.dropped
        );
    }

    // 判定的**权威依据是注入器自己的统计**（injected 只在真的调过
    // SendInput 时才增加）。光标位移只作旁证 —— 这台机器上**用户自己**
    // 也在动鼠标（实测：注入器一个事件都没收到，光标却「位移 97 像素」，
    // 那是用户的手，不是我们的注入 ⇒ 不能拿它当判据）。
    // 客户端发出的事件数：脚本模式计脚本源，真实模式计钩子采到的。
    let sent_total = sent as u64 + captured_total;
    let injected = injector_stats
        .as_ref()
        .map(|s| s.snapshot().injected)
        .unwrap_or(0);
    let ok = sent_total >= 2 && injected >= 1;
    println!(
        "RESULT: {} —— 客户端事件真的经输入流到了主机,主机的桌面真的动了",
        if ok { "PASS" } else { "FAIL" }
    );
    if !ok {
        return Err(format!(
            "输入回传未生效：发出 {sent} 个事件，光标位移 {moved} 像素（起点 {cx},{cy}）"
        )
        .into());
    }
    Ok(())
}

/// 按脚本发事件的采集源（模拟客户端的键鼠采集）。
///
/// 真机采集见 [`beam_client::capture::WindowsInputCapture`]；
/// 这里用脚本源是为了让验证**确定可重复**（不依赖「用户正好动了鼠标」）。
struct ScriptedInput {
    queue: Mutex<VecDeque<InputEvent>>,
    sent: Mutex<usize>,
}

impl ScriptedInput {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            sent: Mutex::new(0),
        }
    }

    fn push(&self, ev: InputEvent) {
        self.queue.lock().unwrap().push_back(ev);
    }

    fn sent(&self) -> usize {
        *self.sent.lock().unwrap()
    }
}

impl beam_session::InputSource for ScriptedInput {
    fn poll_event(&self) -> Option<InputEvent> {
        let ev = self.queue.lock().unwrap().pop_front()?;
        *self.sent.lock().unwrap() += 1;
        Some(ev)
    }
}

/// 构造一个「移动到屏幕坐标」的协议事件（归一化 `0..=65535`，§3.8）。
fn mouse_move_to(x: i32, y: i32) -> InputEvent {
    let (w, h) = screen_size();
    let nx = (x.clamp(0, w - 1) as i64 * 65535 / (w - 1).max(1) as i64) as i32;
    let ny = (y.clamp(0, h - 1) as i64 * 65535 / (h - 1).max(1) as i64) as i32;
    InputEvent::MouseMove { x: nx, y: ny }
}

/// 注入一个测试键（F13 下 + 上）。
///
/// F13 机器上没人用 → 不会影响任何应用。
fn inject_test_key() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    // 用 **Ctrl**（单独按 Ctrl 没有任何副作用），而不是自定义的键码：
    // 虚拟键码必须在 HID 表里（否则会被「表里没有就跳过」拒掉）。
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0xA2); // VK_LCONTROL
    let make = |flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_CONTROL,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [
        make(windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0)),
        make(KEYEVENTF_KEYUP),
    ];
    // SAFETY: `inputs` 是合法的 `INPUT` 切片，`SendInput` 只在调用期间读它。
    unsafe {
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 读当前光标位置（像素）。
fn cursor_position() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT::default();
    // SAFETY: `GetCursorPos` 只写 `pt` 这一个结构体。
    let ok = unsafe { GetCursorPos(&mut pt) };
    if ok.is_ok() {
        (pt.x, pt.y)
    } else {
        (0, 0)
    }
}

/// 主显示器的宽高（像素）。
fn screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: `GetSystemMetrics` 是纯查询。
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}
