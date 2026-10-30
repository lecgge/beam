//! 主机会话（§3.9 `HostSession::start(cfg, deps) -> SessionHandle`）。
//!
//! 主机侧串联：抓屏 → 编码 → net。
//! 平台能力通过 trait 注入（[`crate::event::FramePipeline`] / [`crate::event::VideoSender`]），
//! 因此本 crate 不依赖任何平台 crate（§3.9 边界）。
//!
//! 控制通道（§5 控制 stream 0）通过 [`crate::event::ControlChannel`] 注入：
//! 收到 `Hello` 后做能力协商并回 `HelloAck`，收到 `RequestIdr` / `Feedback` /
//! `Resume` / `Bye` 分别触发 IDR、码率控制、会话恢复与会话关闭（§4.1 时序）。
//!
//! ## 边界：输入注入归 beam-host
//!
//! 输入事件走 §5 的**输入单向流**（客户端 → 主机），接收与注入都在 beam-host
//! （§3.8 的输入任务）。本 crate 不持有输入注入器
//! （[`HostDeps`] 里没有 `input` 字段），因此只有**一处**会注入输入。

use std::sync::{Arc, Weak};
use std::time::Duration;

use beam_proto::{ControlMsg, StatsReport, StreamConfig};

use crate::clock::duration_ms;
use crate::control::{
    caps_from_config, next_session_id, ControlContext, ControlOutcome, ControlRole, PairingConfig,
    STATS_INTERVAL_MS,
};
use crate::error::SessionError;
use crate::event::{
    ControlChannel, FramePipeline, RateControl, SessionEvent, SessionInput, VideoSender,
};
use crate::handle::{SessionHandle, SessionInner, SessionRunner, SessionTimeouts};
use crate::state::{SessionState, StateMachine};

/// 主机会话配置（§3.9）。
#[derive(Debug, Clone)]
pub struct HostSessionConfig {
    /// 串流配置。
    pub stream: StreamConfig,
    /// 重连超时（§3.9 `reconnect_timeout=30s`）。
    pub reconnect_timeout: Duration,
    /// 停滞阈值（§3.9 `stall_threshold=3s`）。
    pub stall_threshold: Duration,
}

impl Default for HostSessionConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            reconnect_timeout: Duration::from_secs(30),
            stall_threshold: Duration::from_secs(3),
        }
    }
}

/// 主机会话依赖（全部为 trait 对象，便于测试）。
///
/// ## 没有 `input` 字段：输入注入归 beam-host
///
/// 输入事件走 §5 的**输入单向流**（客户端 → 主机），由 beam-host 的输入任务
/// 接收并注入平台（§3.8「输入 stream 接收任务 → 有界通道 → 注入线程」）。
/// 会话层不持有输入注入器，因此不存在「会话层注入一次、主机再注入一次」
/// 的重复注入风险。
pub struct HostDeps {
    /// 抓屏 → 编码流水线。
    pub pipeline: Box<dyn FramePipeline>,
    /// 视频 / 控制发送器。
    pub sender: Box<dyn VideoSender>,
}

impl std::fmt::Debug for HostDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDeps").finish_non_exhaustive()
    }
}

/// 主机侧控制通道依赖（§4.1 时序）。
pub struct HostControlDeps {
    /// 控制通道（§5 控制 stream 0，双向可靠）。
    pub control: Box<dyn ControlChannel>,
    /// 配对配置（§3.3）：主机用 PIN 校验、用 token 接受 `PairRequest`。
    pub pairing: PairingConfig,
    /// 码率控制器（§3.2）：收到 `Feedback` 时喂给它。
    pub rate: Option<Box<dyn RateControl>>,
}

impl std::fmt::Debug for HostControlDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostControlDeps")
            .field("pairing", &self.pairing)
            .field("rate", &self.rate.is_some())
            .finish()
    }
}

/// 主机会话驱动。
pub(crate) struct HostRunner {
    machine: StateMachine,
    deps: HostDeps,
    /// 控制通道；未注入时主机退化为「只推帧」模式。
    control: Option<Box<dyn ControlChannel>>,
    inner: Weak<SessionInner>,
    pipeline_started: bool,
    ctrl: ControlContext,
    /// 会话号（§5 `Resume{session_id}`）。
    session_id: u64,
    /// 最后一次收到帧 / 控制消息的时间戳（§4.1「3 s 无数据」）。
    last_data_ms: u64,
    /// 进入 `Reconnecting` 的时间戳（§4.1「30 s 超时」）。
    reconnect_since_ms: Option<u64>,
    /// 上一次统计上报时间戳。
    last_stats_ms: u64,
    /// 本统计周期内已发送的帧数（用于算实测帧率）。
    frames_sent: u32,
    /// 本端统计。
    stats: StatsReport,
}

impl HostRunner {
    fn emit(&self, event: SessionEvent) {
        if let Some(inner) = self.inner.upgrade() {
            inner.emit(event);
        }
    }

    fn set_state(&self, state: SessionState) {
        if let Some(inner) = self.inner.upgrade() {
            if let Ok(mut guard) = inner.state.lock() {
                *guard = state;
            }
        }
    }

    fn goto(&mut self, next: SessionState) -> Result<(), SessionError> {
        match self.machine.transition(next) {
            Ok(state) => {
                self.set_state(state);
                self.emit(SessionEvent::StateChanged { state });
                Ok(())
            }
            Err(e) => {
                self.emit(SessionEvent::Error {
                    code: e.code(),
                    retryable: e.retryable(),
                });
                Err(e)
            }
        }
    }

    fn fail(&mut self, err: &SessionError) {
        self.emit(SessionEvent::Error {
            code: err.code(),
            retryable: err.retryable(),
        });
        if self.machine.state().can_transition_to(SessionState::Error) {
            let _ = self.machine.transition(SessionState::Error);
            self.set_state(SessionState::Error);
            self.emit(SessionEvent::StateChanged {
                state: SessionState::Error,
            });
        }
    }

    fn now_ms(&self) -> u64 {
        self.inner.upgrade().map(|i| i.now_ms()).unwrap_or(0)
    }

    fn timeouts(&self) -> SessionTimeouts {
        self.inner
            .upgrade()
            .map(|i| i.timeouts())
            .unwrap_or_default()
    }

    fn config(&self) -> StreamConfig {
        self.inner.upgrade().map(|i| i.config()).unwrap_or_default()
    }

    /// 发送一条控制消息（§5 控制 stream 0）。
    fn send_control(&mut self, msg: ControlMsg) {
        let result = match self.control.as_mut() {
            Some(c) => c.send_control(msg),
            None => return,
        };
        if let Err(e) = result {
            self.fail(&e);
        }
    }

    /// 把发送队列里没写完的字节冲出去（底层缓冲满时需要续写）。
    fn flush_control(&mut self) {
        let result = match self.control.as_mut() {
            Some(c) => c.flush(),
            None => return,
        };
        if let Err(e) = result {
            self.fail(&e);
        }
    }

    /// 收到任何数据（帧或控制消息）时调用：刷新时间戳、必要时从重连恢复。
    fn on_data(&mut self) {
        self.last_data_ms = self.now_ms();
        if self.machine.state() == SessionState::Reconnecting {
            // §4.1：30 s 内恢复 → 回到 Streaming，并立刻补一个 IDR。
            if self.goto(SessionState::Streaming).is_ok() {
                self.request_idr();
            }
        }
    }

    /// 向编码器请求 IDR（§3.5 `request_idr`、§4.1「重连后主机发 IDR」）。
    fn request_idr(&mut self) {
        self.deps.pipeline.request_idr();
    }

    /// 处理一条控制消息（§4.1）。
    fn handle_control(&mut self, msg: ControlMsg) {
        let outcome: ControlOutcome = self.ctrl.handle(msg);
        self.apply_outcome(outcome);
    }

    /// 把处理结果落到状态机上。
    fn apply_outcome(&mut self, outcome: ControlOutcome) {
        if let Some(err) = outcome.error {
            self.fail(&err);
            return;
        }
        for reply in outcome.replies {
            self.send_control(reply);
        }
        if let Some(agreed) = outcome.negotiated {
            if let Some(inner) = self.inner.upgrade() {
                inner.set_agreed(Some(agreed));
            }
            let config = self.config();
            if self.goto(SessionState::Streaming).is_ok() {
                self.emit(SessionEvent::Negotiated { config });
            }
        }
        if let Some(fingerprint) = outcome.paired {
            self.emit(SessionEvent::Paired {
                fingerprint: fingerprint.to_hex(),
            });
        }
        if let Some(stats) = outcome.stats {
            self.stats = stats.clone();
            self.emit(SessionEvent::Stats(stats));
        }
        if let Some(cfg) = outcome.peer_config {
            if let Some(inner) = self.inner.upgrade() {
                inner.set_config(cfg);
            }
        }
        if let Some(bps) = outcome.bitrate_bps {
            let mut cfg = self.config();
            cfg.bitrate_bps = bps;
            if let Some(inner) = self.inner.upgrade() {
                inner.set_config(cfg);
            }
        }
        if outcome.request_idr {
            self.request_idr();
        }
        if let Some(session_id) = outcome.resumed {
            // §4.1：新连接 / 迁移 → `Resume{session_id}` → 恢复会话并补 IDR。
            self.session_id = session_id;
            if self.machine.state() == SessionState::Reconnecting
                && self.goto(SessionState::Streaming).is_ok()
            {
                self.request_idr();
            }
        }
        if outcome.close {
            self.close_session();
        }
    }

    /// 关闭会话（收到 `Bye` 或本地停止）。
    fn close_session(&mut self) {
        // 保留控制通道：QUIC 连接的最后一个引用被释放时会立即关闭连接，
        // 对端就收不到「会话结束」这条消息了。
        self.deps.pipeline.stop();
        self.pipeline_started = false;
        if self.goto(SessionState::Closed).is_ok() {
            self.emit(SessionEvent::Closed);
        }
    }

    /// 停滞 / 重连超时判定（§4.1、§3.9 `stall_threshold` / `reconnect_timeout`）。
    fn check_deadlines(&mut self) {
        let now = self.now_ms();
        let timeouts = self.timeouts();
        match self.machine.state() {
            SessionState::Streaming => {
                let threshold = duration_ms(timeouts.stall_threshold);
                if threshold == 0 {
                    return;
                }
                if now.saturating_sub(self.last_data_ms) >= threshold
                    && self.goto(SessionState::Reconnecting).is_ok()
                {
                    self.reconnect_since_ms = Some(now);
                }
            }
            SessionState::Reconnecting => {
                let limit = duration_ms(timeouts.reconnect_timeout);
                if limit == 0 {
                    return;
                }
                let since = self.reconnect_since_ms.unwrap_or(now);
                if now.saturating_sub(since) >= limit {
                    let err = SessionError::SessionTimeout {
                        secs: timeouts.reconnect_timeout.as_secs(),
                    };
                    self.emit(SessionEvent::Error {
                        code: err.code(),
                        retryable: err.retryable(),
                    });
                    self.control = None;
                    if self.goto(SessionState::Closed).is_ok() {
                        self.emit(SessionEvent::Closed);
                    }
                }
            }
            _ => {}
        }
    }

    /// 读控制消息（§4.1 控制面）。
    fn drain_control(&mut self) {
        if self.machine.state() == SessionState::Closed {
            return;
        }
        loop {
            let next = self.control.as_mut().and_then(|c| c.recv_control());
            let Some(item) = next else {
                return;
            };
            match item {
                Ok(msg) => {
                    self.on_data();
                    self.handle_control(msg);
                    if self.machine.state() == SessionState::Closed {
                        return;
                    }
                }
                Err(e) => {
                    self.fail(&e);
                    return;
                }
            }
        }
    }

    /// 抓屏 → 编码 → 发送。
    fn pump_frames(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        if !self.pipeline_started {
            if let Err(e) = self.deps.pipeline.start() {
                self.fail(&e);
                return;
            }
            self.pipeline_started = true;
        }
        while let Some(frame) = self.deps.pipeline.poll() {
            if let Err(e) = self.deps.sender.send_frame(frame) {
                self.fail(&e);
                return;
            }
            self.frames_sent += 1;
        }
    }

    /// 统计节拍（§3.11 `session://stats` 500 ms）。
    fn maybe_report_stats(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        let now = self.now_ms();
        let elapsed = now.saturating_sub(self.last_stats_ms);
        if elapsed < STATS_INTERVAL_MS {
            return;
        }
        self.last_stats_ms = now;
        let fps = if elapsed == 0 {
            0
        } else {
            (self.frames_sent as u64 * 1000 / elapsed) as u32
        };
        self.frames_sent = 0;

        let mut report = self.stats.clone();
        report.session_id = self.session_id;
        report.fps = fps;
        report.bitrate_bps = self.config().bitrate_bps;
        self.stats = report.clone();
        self.emit(SessionEvent::Stats(report.clone()));
        self.send_control(ControlMsg::StatsReport(report));
    }
}

impl SessionRunner for HostRunner {
    fn apply(&mut self, input: SessionInput) {
        match input {
            SessionInput::Pairing => {
                let _ = self.goto(SessionState::Pairing);
            }
            SessionInput::Paired { fingerprint } => {
                if self.machine.state() == SessionState::Pairing {
                    self.emit(SessionEvent::Paired { fingerprint });
                }
            }
            SessionInput::Connecting => {
                let _ = self.goto(SessionState::Connecting);
            }
            SessionInput::Connected => {
                let _ = self.goto(SessionState::Negotiating);
            }
            SessionInput::Negotiated(config) => {
                if self.goto(SessionState::Streaming).is_ok() {
                    self.emit(SessionEvent::Negotiated { config });
                }
            }
            SessionInput::Streaming => {
                let _ = self.goto(SessionState::Streaming);
            }
            SessionInput::Stalled => {
                if self.machine.state() == SessionState::Streaming
                    && self.goto(SessionState::Reconnecting).is_ok()
                {
                    self.reconnect_since_ms = Some(self.now_ms());
                }
            }
            SessionInput::Reconnected => {
                if self.machine.state() == SessionState::Reconnecting
                    && self.goto(SessionState::Streaming).is_ok()
                {
                    self.request_idr();
                }
            }
            SessionInput::Failed(code) => {
                let err = SessionError::TransportFailed {
                    reason: format!("会话失败(code={code})"),
                };
                self.fail(&err);
            }
            SessionInput::Stop => {
                // 发出 `Bye` 后保留控制通道，避免连接先于消息关闭。
                self.send_control(ControlMsg::Bye);
                self.deps.pipeline.stop();
                self.pipeline_started = false;
                let _ = self.goto(SessionState::Closed);
            }
            SessionInput::Discovering => {
                let _ = self.goto(SessionState::Discovering);
            }
        }
    }

    fn pump(&mut self) {
        self.check_deadlines();
        self.drain_control();
        self.pump_frames();
        self.maybe_report_stats();
        self.flush_control();
    }

    fn on_update(&mut self, _config: &StreamConfig) {
        // 主机侧的配置由对端的 `StreamConfig` 消息驱动，这里无需额外动作。
    }

    fn on_stop(&mut self) {
        // 同 `SessionInput::Stop`：保留控制通道直到会话彻底释放。
        self.send_control(ControlMsg::Bye);
        self.deps.pipeline.stop();
        self.pipeline_started = false;
    }
}

/// 主机会话（§3.9）。
pub struct HostSession {
    handle: SessionHandle,
}

impl HostSession {
    /// 启动主机会话（§3.9 `HostSession::start(cfg, deps) -> SessionHandle`）。
    ///
    /// 不注入控制通道时主机退化为「只推帧」模式：控制面（§4.1）不可用。
    pub fn start(cfg: HostSessionConfig, deps: HostDeps) -> SessionHandle {
        Self::start_with_control(cfg, deps, None)
    }

    /// 启动主机会话并注入控制通道（§4.1 时序）。
    ///
    /// `ctrl` 为 `None` 时等价于 [`HostSession::start`]。
    pub fn start_with_control(
        cfg: HostSessionConfig,
        deps: HostDeps,
        ctrl: Option<HostControlDeps>,
    ) -> SessionHandle {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let handle = SessionHandle::new(SessionState::Idle, cfg.stream.clone(), tx, rx);
        handle.set_timeouts(SessionTimeouts {
            reconnect_timeout: cfg.reconnect_timeout,
            stall_threshold: cfg.stall_threshold,
        });
        let ctrl = ctrl.unwrap_or(HostControlDeps {
            control: Box::new(NullControlChannel),
            pairing: PairingConfig::default(),
            rate: None,
        });
        let control = ctrl.control;
        let processor = ControlContext::new(
            ControlRole::Host,
            caps_from_config(&cfg.stream),
            ctrl.pairing,
            ctrl.rate,
            handle.inner().clock(),
        );
        let now = handle.now_ms();
        let runner = HostRunner {
            machine: StateMachine::new(),
            deps,
            control: Some(control),
            inner: Arc::downgrade(handle.inner()),
            pipeline_started: false,
            ctrl: processor,
            session_id: next_session_id(),
            last_data_ms: now,
            reconnect_since_ms: None,
            last_stats_ms: now,
            frames_sent: 0,
            stats: StatsReport::default(),
        };
        handle.set_runner(Box::new(runner));
        handle
    }

    /// 会话句柄。
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }
}

/// 空控制通道（未注入控制通道时的主机使用）。
struct NullControlChannel;

impl ControlChannel for NullControlChannel {
    fn send_control(&mut self, _msg: ControlMsg) -> Result<(), SessionError> {
        Ok(())
    }

    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use beam_proto::{EncodedFrame, Feedback};
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakePipeline {
        frames: AtomicUsize,
        started: AtomicUsize,
        idr: AtomicUsize,
    }

    impl FramePipeline for FakePipeline {
        fn start(&mut self) -> Result<(), SessionError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn poll(&mut self) -> Option<EncodedFrame> {
            if self.frames.fetch_add(1, Ordering::SeqCst) < 2 {
                Some(EncodedFrame {
                    frame_id: 1,
                    pts_us: 0,
                    capture_ts_us: 0,
                    is_key: true,
                    slices: vec![bytes::Bytes::from_static(b"x")],
                })
            } else {
                None
            }
        }
        fn request_idr(&mut self) {
            self.idr.fetch_add(1, Ordering::SeqCst);
        }
        fn stop(&mut self) {}
    }

    struct FakeSender {
        sent: AtomicUsize,
    }

    impl VideoSender for FakeSender {
        fn send_frame(&mut self, _f: EncodedFrame) -> Result<(), SessionError> {
            self.sent.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn send_control(&mut self, _msg: ControlMsg) -> Result<(), SessionError> {
            Ok(())
        }
    }

    /// 可脚本化的假控制通道。
    struct FakeControl {
        inbound: std::collections::VecDeque<Result<ControlMsg, SessionError>>,
        sent: std::sync::Arc<std::sync::Mutex<Vec<ControlMsg>>>,
    }

    impl FakeControl {
        fn new() -> Self {
            Self {
                inbound: std::collections::VecDeque::new(),
                sent: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }

    impl ControlChannel for FakeControl {
        fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
            self.sent.lock().expect("锁").push(msg);
            Ok(())
        }
        fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
            self.inbound.pop_front()
        }
    }

    struct RecordingRate {
        seen: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    }

    impl RateControl for RecordingRate {
        fn on_feedback(&mut self, fb: &Feedback) -> u32 {
            self.seen.lock().expect("锁").push(fb.loss);
            8_000_000
        }
    }

    fn deps() -> HostDeps {
        HostDeps {
            pipeline: Box::new(FakePipeline {
                frames: AtomicUsize::new(0),
                started: AtomicUsize::new(0),
                idr: AtomicUsize::new(0),
            }),
            sender: Box::new(FakeSender {
                sent: AtomicUsize::new(0),
            }),
        }
    }

    /// 造一个带假控制通道的主机会话。
    fn host_with(
        control: FakeControl,
        rate: Option<Box<dyn RateControl>>,
    ) -> (
        SessionHandle,
        std::sync::Arc<std::sync::Mutex<Vec<ControlMsg>>>,
    ) {
        let sent = std::sync::Arc::clone(&control.sent);
        let handle = HostSession::start_with_control(
            HostSessionConfig::default(),
            deps(),
            Some(HostControlDeps {
                control: Box::new(control),
                pairing: PairingConfig::default(),
                rate,
            }),
        );
        (handle, sent)
    }

    #[test]
    fn host_session_walks_the_state_machine() {
        let handle = HostSession::start(HostSessionConfig::default(), deps());
        let mut events = handle.events();

        handle.apply(SessionInput::Pairing).expect("应当成功");
        handle
            .apply(SessionInput::Paired {
                fingerprint: "abc".into(),
            })
            .expect("应当成功");
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle
            .apply(SessionInput::Negotiated(StreamConfig::default()))
            .expect("应当成功");
        assert_eq!(handle.state(), SessionState::Streaming);

        handle.pump().expect("应当能推帧");
        handle.apply(SessionInput::Stalled).expect("应当能进入重连");
        assert_eq!(handle.state(), SessionState::Reconnecting);
        handle.apply(SessionInput::Reconnected).expect("应当能恢复");
        assert_eq!(handle.state(), SessionState::Streaming);

        // 至少能收到状态变化事件。
        let ev = futures::executor::block_on(events.next()).expect("应当有事件");
        assert!(matches!(ev, SessionEvent::StateChanged { .. }));
    }

    #[test]
    fn illegal_transition_emits_error() {
        let handle = HostSession::start(HostSessionConfig::default(), deps());
        let mut events = handle.events();
        // Idle -> Streaming 非法。
        handle
            .apply(SessionInput::Streaming)
            .expect("投递本身不失败");
        let mut saw_error = false;
        for _ in 0..4 {
            match futures::executor::block_on(events.next()) {
                Some(SessionEvent::Error { code, .. }) => {
                    assert_eq!(code, 9001);
                    saw_error = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(saw_error, "非法转移应当产生错误事件");
    }

    #[test]
    fn host_answers_hello_with_hello_ack() {
        let mut control = FakeControl::new();
        control
            .inbound
            .push_back(Ok(ControlMsg::Hello(beam_proto::message::Hello {
                ver: beam_proto::ProtocolVersion::CURRENT,
                caps: beam_proto::Caps {
                    codecs: vec![beam_proto::CodecKind::H264],
                    max_res: beam_proto::Resolution::new(1280, 720),
                    max_fps: 30,
                    features: 0,
                },
            })));
        let (handle, sent) = host_with(control, None);
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle.pump().expect("应当能收控制消息");

        let log = sent.lock().expect("锁").clone();
        assert!(
            log.iter().any(|m| matches!(m, ControlMsg::HelloAck { .. })),
            "收到 Hello 必须回 HelloAck"
        );
        assert_eq!(handle.state(), SessionState::Streaming);
        assert!(handle.agreed().is_some());
    }

    #[test]
    fn host_request_idr_reaches_pipeline() {
        let mut control = FakeControl::new();
        control.inbound.push_back(Ok(ControlMsg::RequestIdr));
        let pipeline = std::sync::Arc::new(AtomicUsize::new(0));
        let handle = HostSession::start_with_control(
            HostSessionConfig::default(),
            HostDeps {
                pipeline: Box::new(CountingPipeline {
                    idr: std::sync::Arc::clone(&pipeline),
                }),
                sender: Box::new(FakeSender {
                    sent: AtomicUsize::new(0),
                }),
            },
            Some(HostControlDeps {
                control: Box::new(control),
                pairing: PairingConfig::default(),
                rate: None,
            }),
        );
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle.pump().expect("应当能收控制消息");
        assert_eq!(pipeline.load(Ordering::SeqCst), 1, "必须请求 IDR");
    }

    #[test]
    fn host_feeds_feedback_to_rate_controller() {
        let mut control = FakeControl::new();
        control.inbound.push_back(Ok(ControlMsg::Feedback(Feedback {
            loss: 77,
            rtt: Duration::from_millis(4),
            recv_rate: 20_000_000,
        })));
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (handle, _sent) = host_with(
            control,
            Some(Box::new(RecordingRate {
                seen: std::sync::Arc::clone(&seen),
            })),
        );
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle.pump().expect("应当能收控制消息");
        assert_eq!(seen.lock().expect("锁").as_slice(), &[77]);
    }

    #[test]
    fn host_resume_restores_streaming_and_requests_idr() {
        let mut control = FakeControl::new();
        control
            .inbound
            .push_back(Ok(ControlMsg::Resume { session_id: 7 }));
        let pipeline = std::sync::Arc::new(AtomicUsize::new(0));
        let handle = HostSession::start_with_control(
            HostSessionConfig::default(),
            HostDeps {
                pipeline: Box::new(CountingPipeline {
                    idr: std::sync::Arc::clone(&pipeline),
                }),
                sender: Box::new(FakeSender {
                    sent: AtomicUsize::new(0),
                }),
            },
            Some(HostControlDeps {
                control: Box::new(control),
                pairing: PairingConfig::default(),
                rate: None,
            }),
        );
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle
            .apply(SessionInput::Negotiated(StreamConfig::default()))
            .expect("应当成功");
        handle.apply(SessionInput::Stalled).expect("应当能进入重连");
        assert_eq!(handle.state(), SessionState::Reconnecting);
        handle.pump().expect("应当能收控制消息");
        assert_eq!(
            handle.state(),
            SessionState::Streaming,
            "Resume 必须恢复会话"
        );
        assert_eq!(pipeline.load(Ordering::SeqCst), 1, "恢复后必须补 IDR");
    }

    #[test]
    fn host_bye_closes_session() {
        let mut control = FakeControl::new();
        control.inbound.push_back(Ok(ControlMsg::Bye));
        let (handle, _sent) = host_with(control, None);
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle.pump().expect("应当能收控制消息");
        assert_eq!(handle.state(), SessionState::Closed);
    }

    #[test]
    fn host_stall_and_reconnect_timeout() {
        let control = FakeControl::new();
        let (handle, _sent) = host_with(control, None);
        let mut events = handle.events();
        let clock = std::sync::Arc::new(ManualClock::new());
        handle.set_clock(std::sync::Arc::clone(&clock) as std::sync::Arc<dyn crate::clock::Clock>);
        handle.apply(SessionInput::Connecting).expect("应当成功");
        handle.apply(SessionInput::Connected).expect("应当成功");
        handle
            .apply(SessionInput::Negotiated(StreamConfig::default()))
            .expect("应当成功");
        assert_eq!(handle.state(), SessionState::Streaming);

        clock.advance(Duration::from_secs(4));
        handle.pump().expect("应当能推进");
        assert_eq!(handle.state(), SessionState::Reconnecting);

        clock.advance(Duration::from_secs(31));
        handle.pump().expect("应当能推进");
        assert_eq!(handle.state(), SessionState::Closed);

        let mut saw_timeout = false;
        for _ in 0..8 {
            match futures::executor::block_on(events.next()) {
                Some(SessionEvent::Error { code: 9003, .. }) => {
                    saw_timeout = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(saw_timeout, "必须报 E9003 会话超时");
    }

    /// 只统计 IDR 次数的流水线。
    struct CountingPipeline {
        idr: std::sync::Arc<AtomicUsize>,
    }

    impl FramePipeline for CountingPipeline {
        fn start(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
        fn poll(&mut self) -> Option<EncodedFrame> {
            None
        }
        fn request_idr(&mut self) {
            self.idr.fetch_add(1, Ordering::SeqCst);
        }
        fn stop(&mut self) {}
    }
}
