//! 假实现集成测试（§8「集成:假 capture/encoder 在本机回环跑完整会话」）。
//!
//! 全部用**假实现**：假抓屏 / 假编码器 / 假传输，
//! 不依赖 QUIC、不依赖平台、不依赖外网，可重复运行。
//!
//! ## 覆盖什么
//!
//! | 场景 | 断言 |
//! |---|---|
//! | 正常建链 → 协商 → 串流 → 结束 | 两端进 `Streaming`；`Agreed` 取双方交集；`Negotiated` 事件带交集配置；画面到达消费者；`Bye` 后主机 `Closed` |
//! | 停滞 → 重连 → 恢复 | 3 s 无数据进 `Reconnecting`；**重建了新连接**；发 `Resume{session_id}`；新连接上重新 `Hello` 协商；主机补 IDR；客户端恢复 `Streaming` 并请求 IDR；重连期间**保留最后一帧**；全程无 `E9001` |
//! | 重连超时 → `Closed` | 建连一直失败时 30 s 后进 `Closed` 并报 `E9003` |
//! | 丢帧 | 产出 `SessionEvent::FramesDropped`，**不进 `Error` 态** |
//!
//! 真实 QUIC 的回环测试在 `tests/loopback.rs`（另一个目标）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beam_proto::{ControlMsg, EncodedFrame, InputEvent, StreamConfig};
use beam_session::{
    ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, Clock, ControlChannel,
    FrameConsumer, FramePipeline, HostControlDeps, HostDeps, HostSession, HostSessionConfig,
    ManualClock, PairingConfig, PeerAddr, SessionError, SessionEvent, SessionEventStream,
    SessionHandle, SessionInput, SessionState, Transport, TransportFactory, VideoSender,
};
use bytes::Bytes;
use futures::{FutureExt, StreamExt};

/// 客户端能力：720p30 / H.264。
fn client_stream() -> StreamConfig {
    StreamConfig {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_bps: 8_000_000,
        codec: beam_proto::CodecKind::H264,
        audio_enabled: false,
        features: 0,
    }
}

/// 主机能力：1080p60 / H.264（比客户端大 → 协商结果应当取**客户端**那一份）。
fn host_stream() -> StreamConfig {
    StreamConfig {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_bps: 20_000_000,
        codec: beam_proto::CodecKind::H264,
        audio_enabled: false,
        features: 0,
    }
}

/// 假端点之间的内存「线」。
#[derive(Clone, Default)]
struct Wire {
    /// 客户端 → 主机的控制消息。
    to_host: Arc<Mutex<VecDeque<ControlMsg>>>,
    /// 主机 → 客户端的控制消息。
    to_client: Arc<Mutex<VecDeque<ControlMsg>>>,
    /// 主机 → 客户端的画面帧。
    frames: Arc<Mutex<VecDeque<EncodedFrame>>>,
    /// 客户端发出的控制消息日志（断言用）。
    client_sent: Arc<Mutex<Vec<ControlMsg>>>,
    /// 待上报的丢帧数（§3.2 **E2004**）。
    dropped: Arc<AtomicU32>,
}

/// 假抓屏 + 假编码器：产出若干关键帧，并统计 IDR 请求次数。
struct FakePipeline {
    remaining: usize,
    is_key: bool,
    idr_requests: Arc<AtomicUsize>,
}

impl FramePipeline for FakePipeline {
    fn start(&mut self) -> Result<(), SessionError> {
        Ok(())
    }

    fn poll(&mut self) -> Option<EncodedFrame> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some(EncodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            is_key: self.is_key,
            slices: vec![Bytes::from_static(b"fake-frame")],
        })
    }

    fn request_idr(&mut self) {
        self.idr_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn stop(&mut self) {}
}

/// 假发送器：画面直接进内存队列。
struct FakeSender {
    frames: Arc<Mutex<VecDeque<EncodedFrame>>>,
}

impl VideoSender for FakeSender {
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        self.frames.lock().expect("锁").push_back(f);
        Ok(())
    }

    fn send_control(&mut self, _msg: ControlMsg) -> Result<(), SessionError> {
        Ok(())
    }
}

/// 主机侧假控制通道：收发都走内存队列。
struct FakeHostControl {
    wire: Wire,
}

impl ControlChannel for FakeHostControl {
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        self.wire.to_client.lock().expect("锁").push_back(msg);
        Ok(())
    }

    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        self.wire.to_host.lock().expect("锁").pop_front().map(Ok)
    }
}

/// 客户端侧假传输：控制走内存队列，画面从队列取。
struct FakeClientTransport {
    wire: Wire,
}

impl ControlChannel for FakeClientTransport {
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        self.wire.client_sent.lock().expect("锁").push(msg.clone());
        self.wire.to_host.lock().expect("锁").push_back(msg);
        Ok(())
    }

    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        self.wire.to_client.lock().expect("锁").pop_front().map(Ok)
    }
}

impl Transport for FakeClientTransport {
    fn send_frame(&mut self, _f: EncodedFrame) -> Result<(), SessionError> {
        Ok(())
    }

    fn send_input(&mut self, _ev: InputEvent) -> Result<(), SessionError> {
        Ok(())
    }

    fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
        self.wire.frames.lock().expect("锁").pop_front().map(Ok)
    }

    fn take_dropped_frames(&mut self) -> u32 {
        self.wire.dropped.swap(0, Ordering::SeqCst)
    }
}

/// 假传输工厂：每次 `connect()` 都返回一条**新**的假连接。
struct FakeFactory {
    wire: Wire,
    connects: Arc<AtomicUsize>,
}

impl TransportFactory for FakeFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeClientTransport {
            wire: self.wire.clone(),
        }))
    }
}

/// 首连成功、之后一直失败的传输工厂（模拟「连着连着主机不可达」）。
struct FlakyFactory {
    wire: Wire,
    connects: Arc<AtomicUsize>,
}

impl TransportFactory for FlakyFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        let n = self.connects.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            return Ok(Box::new(FakeClientTransport {
                wire: self.wire.clone(),
            }));
        }
        Err(SessionError::TransportFailed {
            reason: "主机不可达".to_string(),
        })
    }
}

/// 客户端侧假消费者：记录收到的画面帧。
struct FakeConsumer {
    received: Arc<Mutex<Vec<EncodedFrame>>>,
}

impl FrameConsumer for FakeConsumer {
    fn consume(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        self.received.lock().expect("锁").push(f);
        Ok(())
    }
}

/// 一对跑在假实现上的会话。
struct Harness {
    client: SessionHandle,
    host: SessionHandle,
    wire: Wire,
    clock: Arc<ManualClock>,
    /// 客户端建过多少条连接（1 = 首连，2 = 重连后新建）。
    connects: Arc<AtomicUsize>,
    /// 主机编码器被要求补 IDR 的次数。
    idr_requests: Arc<AtomicUsize>,
    /// 客户端收到的画面帧。
    received: Arc<Mutex<Vec<EncodedFrame>>>,
}

impl Harness {
    /// 推进一步（客户端 + 主机）。
    fn pump(&self) {
        self.client.pump().expect("客户端推进");
        self.host.pump().expect("主机推进");
    }

    /// 两端一起推进，直到 `done` 为真。
    fn pump_until(&self, done: impl Fn(&Harness) -> bool) -> bool {
        for _ in 0..50 {
            self.pump();
            if done(self) {
                return true;
            }
        }
        false
    }

    /// 非阻塞抽干事件流。
    fn drain(stream: &mut SessionEventStream) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        while let Some(Some(ev)) = stream.next().now_or_never() {
            out.push(ev);
        }
        out
    }
}

/// 起一对假会话。
fn harness_with(
    wire: Wire,
    client_cfg: ClientSessionConfig,
    host_cfg: HostSessionConfig,
    factory: Box<dyn TransportFactory>,
    connects: Arc<AtomicUsize>,
) -> Harness {
    let idr_requests = Arc::new(AtomicUsize::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::new(ManualClock::new());

    let client = ClientSession::connect_with_control(
        PeerAddr {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: None,
        },
        client_cfg,
        ClientDeps {
            consumer: Some(Box::new(FakeConsumer {
                received: Arc::clone(&received),
            })),
            transport: Some(factory),
        },
        ClientControlDeps::default(),
    );

    let host = HostSession::start_with_control(
        host_cfg,
        HostDeps {
            pipeline: Box::new(FakePipeline {
                remaining: 16,
                is_key: true,
                idr_requests: Arc::clone(&idr_requests),
            }),
            sender: Box::new(FakeSender {
                frames: Arc::clone(&wire.frames),
            }),
        },
        Some(HostControlDeps {
            control: Box::new(FakeHostControl { wire: wire.clone() }),
            pairing: PairingConfig::default(),
            rate: None,
        }),
    );
    // 主机必须先走到 `Negotiating`，收到客户端的 `Hello` 才能进 `Streaming`。
    host.apply(SessionInput::Connecting)
        .expect("主机应当能进入连接态");
    host.apply(SessionInput::Connected)
        .expect("主机应当能进入协商态");

    // 换假时钟：时间由测试自己推进（§3.9「超时判定必须可用假时钟驱动」）。
    client.set_clock(Arc::clone(&clock) as Arc<dyn Clock>);
    host.set_clock(Arc::clone(&clock) as Arc<dyn Clock>);

    Harness {
        client,
        host,
        wire,
        clock,
        connects,
        idr_requests,
        received,
    }
}

/// 起一对假会话（连接总是成功）。
fn harness() -> Harness {
    let wire = Wire::default();
    let connects = Arc::new(AtomicUsize::new(0));
    harness_with(
        wire.clone(),
        ClientSessionConfig {
            stream: client_stream(),
            ..ClientSessionConfig::default()
        },
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        Box::new(FakeFactory {
            wire,
            connects: Arc::clone(&connects),
        }),
        connects,
    )
}

/// §4.1：正常建链 → 能力协商 → 串流 → 结束。
#[test]
fn fake_session_negotiates_streams_and_ends() {
    let h = harness();
    let mut client_events = h.client.events();
    let mut host_events = h.host.events();

    assert!(
        h.pump_until(|x| x.client.state() == SessionState::Streaming
            && x.host.state() == SessionState::Streaming),
        "两端都应当进入串流态（客户端={}，主机={}）",
        h.client.state(),
        h.host.state()
    );

    // 能力协商真的跑了一次：结果取双方交集（§3.1）。
    let agreed = h.client.agreed().expect("客户端必须完成能力协商");
    assert_eq!(agreed.width, 1280, "分辨率取双方较小值");
    assert_eq!(agreed.height, 720);
    assert_eq!(agreed.fps, 30, "帧率取双方较小值");
    assert!(h.host.agreed().is_some(), "主机也必须拿到协商结果");

    // 客户端的 `Negotiated` 事件带**交集**配置。
    let client_events = Harness::drain(&mut client_events);
    let client_cfg = client_events
        .iter()
        .find_map(|ev| match ev {
            SessionEvent::Negotiated { config } => Some(config.clone()),
            _ => None,
        })
        .expect("客户端必须收到 Negotiated 事件");
    assert_eq!(client_cfg.width, 1280);
    assert_eq!(client_cfg.height, 720);
    assert_eq!(client_cfg.fps, 30);

    // 主机的 `Negotiated` 事件同样带**交集**配置（不是本机原始配置）。
    let host_events = Harness::drain(&mut host_events);
    let host_cfg = host_events
        .iter()
        .find_map(|ev| match ev {
            SessionEvent::Negotiated { config } => Some(config.clone()),
            _ => None,
        })
        .expect("主机必须发出 Negotiated 事件");
    assert_eq!(host_cfg.width, 1280, "主机必须按协商结果出流");
    assert_eq!(host_cfg.height, 720);
    assert_eq!(host_cfg.fps, 30);
    assert_eq!(h.host.config().width, 1280, "主机会话配置应当是协商结果");

    // 画面真的到了消费者。
    assert!(
        h.pump_until(|x| !x.received.lock().expect("锁").is_empty()),
        "客户端必须收到主机的画面"
    );

    // 结束：客户端发 `Bye` → 主机关闭会话。
    h.client.apply(SessionInput::Stop).expect("应当能停止");
    h.pump();
    assert_eq!(
        h.host.state(),
        SessionState::Closed,
        "收到 Bye 后主机必须关闭"
    );

    // 会话结束后依然能查汇总（§3.9「统计汇总」）。
    let summary = h.client.summary();
    assert!(summary.frames > 0, "汇总里必须有帧数");
    assert_eq!(summary.state, SessionState::Closed);
    assert_eq!(summary.reconnects, 0);
}

/// §4.1「3 s 无帧 → Reconnecting」→ 重建连接 + `Resume` → 恢复串流。
#[test]
fn fake_session_reconnects_after_stall_and_recovers() {
    let h = harness();
    let mut client_events = h.client.events();

    assert!(
        h.pump_until(|x| x.client.state() == SessionState::Streaming),
        "先要进入串流态"
    );
    assert_eq!(h.connects.load(Ordering::SeqCst), 1, "首连建过一条连接");

    // 3 s 无数据（§4.1）→ 进 `Reconnecting`。
    h.clock.advance(Duration::from_secs(4));
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.client.state(),
        SessionState::Reconnecting,
        "超过 stall_threshold 必须进入重连"
    );

    // 重连期间保留最后一帧（产品方案 §3 第 5 步）。
    let info = h.client.reconnecting().expect("必须能查到重连信息");
    assert!(info.keep_last_frame, "重连期间必须保留最后一帧");
    assert_eq!(info.elapsed_ms, 0, "刚进重连，重连时长从 0 开始算");

    // 真的重建了一条**新**连接，并在上面重新协商 + 请求恢复会话。
    assert_eq!(
        h.connects.load(Ordering::SeqCst),
        2,
        "重连必须重建一条新连接"
    );
    let sent = h.wire.client_sent.lock().expect("锁").clone();
    assert!(
        sent.iter().any(|m| matches!(m, ControlMsg::Resume { .. })),
        "重连必须发 Resume{{session_id}}"
    );
    assert!(
        sent.iter()
            .filter(|m| matches!(m, ControlMsg::Hello(_)))
            .count()
            >= 2,
        "新连接上必须重新做一次能力协商（Hello）"
    );
    assert!(
        !sent.iter().any(|m| matches!(
            m,
            ControlMsg::PairRequest { .. } | ControlMsg::PairChallenge { .. }
        )),
        "重连**不得**重走配对：信任已经建立过"
    );

    // 主机侧：收到 `Hello` / `Resume` → 恢复会话并补一个 IDR（§4.1）。
    h.host.pump().expect("主机推进");
    assert!(
        h.idr_requests.load(Ordering::SeqCst) >= 1,
        "重连后主机必须补 IDR"
    );

    // 客户端收到 `HelloAck` → 回到 `Streaming`，并请求一次 IDR。
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.client.state(),
        SessionState::Streaming,
        "重连成功后必须回到串流态"
    );
    assert!(h.client.reconnecting().is_none(), "恢复后不再是重连态");
    let sent = h.wire.client_sent.lock().expect("锁").clone();
    assert!(
        sent.iter().any(|m| matches!(m, ControlMsg::RequestIdr)),
        "重连后客户端必须请求 IDR"
    );

    // 恢复过程不得冒出非法转移（`E9001`）。
    let events = Harness::drain(&mut client_events);
    assert!(
        !events
            .iter()
            .any(|ev| matches!(ev, SessionEvent::Error { code: 9001, .. })),
        "重连恢复不得报非法转移：{events:?}"
    );

    // 统计汇总记下了这次重连。
    let summary = h.client.summary();
    assert_eq!(summary.reconnects, 1, "汇总里必须记一次重连");
    assert_eq!(summary.state, SessionState::Streaming);
}

/// §4.1「30 s 超时 → Closed」：主机一直连不上时回到列表。
#[test]
fn fake_session_reconnect_timeout_closes_session() {
    let wire = Wire::default();
    let connects = Arc::new(AtomicUsize::new(0));
    let h = harness_with(
        wire.clone(),
        ClientSessionConfig {
            stream: client_stream(),
            ..ClientSessionConfig::default()
        },
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        // 首连成功，之后一直失败（模拟「连着连着主机不可达」）。
        Box::new(FlakyFactory {
            wire,
            connects: Arc::clone(&connects),
        }),
        connects,
    );
    let mut client_events = h.client.events();

    assert!(
        h.pump_until(|x| x.client.state() == SessionState::Streaming),
        "先要进入串流态"
    );

    // 3 s 无数据 → `Reconnecting`；重建连接失败 → 保持重连态。
    h.clock.advance(Duration::from_secs(4));
    h.client.pump().expect("客户端推进");
    assert_eq!(h.client.state(), SessionState::Reconnecting);
    assert!(
        h.connects.load(Ordering::SeqCst) >= 2,
        "重连必须尝试重建连接"
    );
    h.clock.advance(Duration::from_secs(5));
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.client.state(),
        SessionState::Reconnecting,
        "重建失败时必须留在重连态继续等"
    );

    // 30 s 窗口用尽 → `Closed`（§4.1「超时回到列表」）。
    h.clock.advance(Duration::from_secs(31));
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.client.state(),
        SessionState::Closed,
        "超过 reconnect_timeout 必须关闭会话"
    );

    let events = Harness::drain(&mut client_events);
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, SessionEvent::Error { code: 9003, .. })),
        "必须报 E9003 会话超时：{events:?}"
    );
    assert!(
        events.iter().any(|ev| matches!(ev, SessionEvent::Closed)),
        "必须发会话结束事件"
    );
}

/// §3.2 **E2004**：丢帧只上报事件，**不进 `Error` 态**。
#[test]
fn fake_session_reports_dropped_frames_without_error_state() {
    let h = harness();
    let mut client_events = h.client.events();

    assert!(
        h.pump_until(|x| x.client.state() == SessionState::Streaming),
        "先要进入串流态"
    );

    // 传输层统计到 5 帧过期丢弃。
    h.wire.dropped.store(5, Ordering::SeqCst);
    h.client.pump().expect("客户端推进");

    let events = Harness::drain(&mut client_events);
    let dropped = events.iter().find_map(|ev| match ev {
        SessionEvent::FramesDropped { count, code } => Some((*count, *code)),
        _ => None,
    });
    assert_eq!(
        dropped,
        Some((5, beam_session::FRAME_EXPIRED_CODE)),
        "丢帧必须原样上报：{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|ev| matches!(ev, SessionEvent::Error { .. })),
        "丢帧不是会话错误：{events:?}"
    );
    assert_eq!(
        h.client.state(),
        SessionState::Streaming,
        "丢帧不得改变会话状态"
    );
    assert_eq!(h.client.summary().dropped_frames, 5, "汇总里必须记丢帧");
}

/// §4.1：重建了连接但**握手没成功**（对端不回 `HelloAck`）时，
/// 不能干等到超时——过了一个 `stall_threshold` 就要再重建一条。
#[test]
fn fake_session_rebuilds_again_when_handshake_does_not_complete() {
    let h = harness();
    assert!(
        h.pump_until(|x| x.client.state() == SessionState::Streaming),
        "先要进入串流态"
    );
    assert_eq!(h.connects.load(Ordering::SeqCst), 1);

    // 3 s 无数据 → `Reconnecting`，重建第 1 条连接。
    h.clock.advance(Duration::from_secs(4));
    h.client.pump().expect("客户端推进");
    assert_eq!(h.client.state(), SessionState::Reconnecting);
    assert_eq!(h.connects.load(Ordering::SeqCst), 2, "先重建一条");

    // **不**推进主机：客户端收不到 `HelloAck`，握手没完成。
    // 再过一个 `stall_threshold` 后必须再重建一条，而不是干等到 30 s 超时。
    h.clock.advance(Duration::from_secs(4));
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.connects.load(Ordering::SeqCst),
        3,
        "握手没成功时必须再重建一条连接"
    );
    assert_eq!(
        h.client.state(),
        SessionState::Reconnecting,
        "还没恢复前必须留在重连态"
    );
}

/// §3.9 配置项：`stall_threshold` / `reconnect_timeout` 真的生效，不是写死的常量。
#[test]
fn fake_session_honours_custom_timeouts() {
    let wire = Wire::default();
    let connects = Arc::new(AtomicUsize::new(0));
    let h = harness_with(
        wire.clone(),
        ClientSessionConfig {
            stream: client_stream(),
            // 自定义阈值：1 s 停滞、5 s 超时。
            stall_threshold: Duration::from_secs(1),
            reconnect_timeout: Duration::from_secs(5),
            ..ClientSessionConfig::default()
        },
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        Box::new(FakeFactory {
            wire,
            connects: Arc::clone(&connects),
        }),
        connects,
    );

    assert!(
        h.pump_until(|x| x.client.state() == SessionState::Streaming),
        "先要进入串流态"
    );
    assert_eq!(h.client.timeouts().stall_threshold, Duration::from_secs(1));
    assert_eq!(
        h.client.timeouts().reconnect_timeout,
        Duration::from_secs(5)
    );

    // 1 s 停滞就进重连（默认阈值是 3 s）。
    h.clock.advance(Duration::from_millis(1200));
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.client.state(),
        SessionState::Reconnecting,
        "自定义 stall_threshold 必须生效"
    );

    // 5 s 超时就关闭（默认是 30 s）。
    h.clock.advance(Duration::from_secs(6));
    h.client.pump().expect("客户端推进");
    assert_eq!(
        h.client.state(),
        SessionState::Closed,
        "自定义 reconnect_timeout 必须生效"
    );
}
