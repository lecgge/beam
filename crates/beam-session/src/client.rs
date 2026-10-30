//! 客户端会话（§3.9 `ClientSession::connect(peer, cfg) -> SessionHandle`）。
//!
//! 客户端侧串联：net → decode → render；并把输入事件回传主机。
//! 与主机侧一样，全部依赖通过 trait 注入（§3.9 边界）。
//!
//! 控制通道（§5 控制 stream 0）由 [`Transport`] 承载：
//! 连接建立后客户端按 §4.1 时序发 `PairRequest` / `PairChallenge`，
//! 配对完成后发 [`ControlMsg::Hello`]，收到 [`ControlMsg::HelloAck`]
//! 才真正进入串流态——能力协商**真的跑一次**，不再只是状态机跳步。
//!
//! ## 首帧策略（§4.1「视频 datagram(首帧 IDR)」）
//!
//! 客户端收到的**第一帧**必须是关键帧。首帧不是关键帧时，
//! 客户端发**一次** `RequestIdr` 让主机补一个 IDR（不重复发）。
//!
//! ## 丢帧上报（§3.2 **E2004**）
//!
//! 传输层统计到的过期帧由 [`Transport::take_dropped_frames`] 取出，
//! 会上报成 [`SessionEvent::FramesDropped`]——**不**把会话打进 `Error` 态。

use std::sync::{Arc, Weak};
use std::time::Duration;

use beam_proto::message::Hello;
use beam_proto::{ControlMsg, EncodedFrame, Feedback, ProtocolVersion, StatsReport, StreamConfig};

use crate::clock::duration_ms;
use crate::control::{
    caps_from_config, config_from_agreed, next_session_id, ControlContext, ControlOutcome,
    ControlRole, PairingConfig, STATS_INTERVAL_MS,
};
use crate::error::SessionError;
use crate::event::{
    FrameConsumer, RateControl, SessionEvent, SessionInput, Transport, TransportFactory,
};
use crate::handle::{SessionHandle, SessionInner, SessionRunner, SessionTimeouts};
use crate::state::{SessionState, StateMachine};

/// 要连接的主机。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerAddr {
    /// 主机地址。
    pub host: String,
    /// 主机端口。
    pub port: u16,
    /// 已固定的对端指纹（十六进制）。
    pub fingerprint: Option<String>,
}

/// 客户端会话配置（§3.9）。
pub struct ClientSessionConfig {
    /// 串流配置。
    pub stream: StreamConfig,
    /// 重连超时（§3.9 `reconnect_timeout=30s`）。
    pub reconnect_timeout: Duration,
    /// 停滞阈值（§3.9 `stall_threshold=3s`）。
    pub stall_threshold: Duration,
    /// 传输工厂（可选）。
    ///
    /// 未注入时 [`ClientSession::connect`] 会把会话推进到 `Error` 并给出 `E9002`，
    /// 因为没有传输就无法建立连接——这比静默挂起更容易定位问题。
    pub transport: Option<Box<dyn TransportFactory>>,
}

impl Default for ClientSessionConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            reconnect_timeout: Duration::from_secs(30),
            stall_threshold: Duration::from_secs(3),
            transport: None,
        }
    }
}

impl std::fmt::Debug for ClientSessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSessionConfig")
            .field("stream", &self.stream)
            .field("reconnect_timeout", &self.reconnect_timeout)
            .field("stall_threshold", &self.stall_threshold)
            .field("transport", &self.transport.is_some())
            .finish()
    }
}

/// 客户端会话依赖。
#[derive(Default)]
pub struct ClientDeps {
    /// 解码 + 上屏。
    pub consumer: Option<Box<dyn FrameConsumer>>,
    /// 传输工厂。
    pub transport: Option<Box<dyn TransportFactory>>,
}

impl std::fmt::Debug for ClientDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientDeps")
            .field("consumer", &self.consumer.is_some())
            .field("transport", &self.transport.is_some())
            .finish()
    }
}

/// 客户端侧控制通道依赖（§4.1 时序）。
#[derive(Default)]
pub struct ClientControlDeps {
    /// 配对配置（§3.3）。留空表示本次连接不需要配对。
    pub pairing: PairingConfig,
    /// 码率控制器（§3.2）。收到 `Feedback` 时喂给它。
    pub rate: Option<Box<dyn RateControl>>,
}

impl std::fmt::Debug for ClientControlDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientControlDeps")
            .field("pairing", &self.pairing)
            .field("rate", &self.rate.is_some())
            .finish()
    }
}

/// 客户端会话驱动。
pub(crate) struct ClientRunner {
    machine: StateMachine,
    peer: PeerAddr,
    consumer: Option<Box<dyn FrameConsumer>>,
    transport: Option<Box<dyn Transport>>,
    inner: Weak<SessionInner>,
    ctrl: ControlContext,
    /// 会话号（§5 `Resume{session_id}`）。
    session_id: u64,
    /// 最后一次收到帧 / 控制消息的时间戳（§4.1「3 s 无数据」）。
    last_data_ms: u64,
    /// 进入 `Reconnecting` 的时间戳（§4.1「30 s 超时」）。
    reconnect_since_ms: Option<u64>,
    /// 是否已经发起过握手。
    handshake_started: bool,
    /// 是否已经发出 `Hello`。
    hello_sent: bool,
    /// 是否已经收到过第一帧（§4.1「首帧 IDR」策略）。
    first_frame_seen: bool,
    /// 是否已经因为「首帧非 IDR」发出过一次 IDR 请求。
    idr_requested: bool,
    /// 上一次统计上报时间戳。
    last_stats_ms: u64,
    /// 本端统计。
    stats: StatsReport,
}

impl ClientRunner {
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
        let result = match self.transport.as_mut() {
            Some(t) => t.send_control(msg),
            None => return,
        };
        if let Err(e) = result {
            self.fail(&e);
        }
    }

    /// 把发送队列里没写完的字节冲出去（底层缓冲满时需要续写）。
    fn flush_control(&mut self) {
        let result = match self.transport.as_mut() {
            Some(t) => t.flush(),
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
            // §4.1：30 s 内恢复 → 回到 Streaming，并请求 IDR。
            if self.goto(SessionState::Streaming).is_ok() {
                self.request_idr();
            }
        }
    }

    /// 请求主机发 IDR（§4.1「重连后主机发 IDR」）。
    fn request_idr(&mut self) {
        self.send_control(ControlMsg::RequestIdr);
    }

    /// §4.1「视频 datagram(首帧 IDR)」：**首帧必须是关键帧**。
    ///
    /// 收到第一帧时若它不是关键帧，说明主机这一轮没有从 IDR 开始
    /// （例如重连后补发的不是关键帧），解码器因此拿不到参考帧——
    /// 客户端必须请求一次 IDR。
    ///
    /// 请求**只发一次**：首帧之后的帧不再重复触发（否则会刷请求）。
    fn check_first_frame_is_idr(&mut self, frame: &EncodedFrame) {
        if self.first_frame_seen {
            return;
        }
        self.first_frame_seen = true;
        if frame.is_key {
            return;
        }
        if self.idr_requested {
            return;
        }
        self.idr_requested = true;
        tracing::warn!(
            frame_id = frame.frame_id,
            "首帧不是 IDR(§4.1 要求首帧 IDR),请求一次 IDR"
        );
        self.request_idr();
    }

    /// 发起握手：先配对，配对完成后发 `Hello`（§4.1）。
    fn start_handshake(&mut self) {
        if self.handshake_started {
            return;
        }
        self.handshake_started = true;
        match self.ctrl.start_pairing() {
            // 有配对消息就等 `PairOk` 回来再发 `Hello`。
            Some(msg) => self.send_control(msg),
            None => self.send_hello(),
        }
    }

    /// 发 `Hello{ver,caps}`（§5）。
    fn send_hello(&mut self) {
        if self.hello_sent {
            return;
        }
        self.hello_sent = true;
        let hello = ControlMsg::Hello(Hello {
            ver: ProtocolVersion::CURRENT,
            caps: self.ctrl.local_caps().clone(),
        });
        self.send_control(hello);
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
            let config = config_from_agreed(&agreed, &self.config());
            if let Some(inner) = self.inner.upgrade() {
                inner.set_agreed(Some(agreed));
            }
            if self.goto(SessionState::Streaming).is_ok() {
                self.emit(SessionEvent::Negotiated { config });
            }
        }
        if let Some(fingerprint) = outcome.paired {
            self.emit(SessionEvent::Paired {
                fingerprint: fingerprint.to_hex(),
            });
            // 配对完成 → 发 `Hello`（§4.1 时序）。
            self.send_hello();
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
            // 客户端不产生画面，忽略主机的 IDR 请求。
        }
        if let Some(session_id) = outcome.resumed {
            self.session_id = session_id;
        }
        if outcome.close {
            self.close_session();
        }
    }

    /// 关闭会话（收到 `Bye` 或本地停止）。
    fn close_session(&mut self) {
        self.transport = None;
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
                    self.transport = None;
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
            let next = self.transport.as_mut().and_then(|t| t.recv_control());
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

    /// 读帧并交给解码 + 上屏。
    fn pump_frames(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        loop {
            let next = self.transport.as_mut().and_then(|t| t.recv_frame());
            let Some(item) = next else {
                return;
            };
            match item {
                Ok(frame) => {
                    self.on_data();
                    // §4.1：首帧必须是 IDR；不是就请求一次。
                    self.check_first_frame_is_idr(&frame);
                    let Some(consumer) = self.consumer.as_mut() else {
                        continue;
                    };
                    if let Err(e) = consumer.consume(frame) {
                        self.fail(&e);
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

    /// 把传输层统计到的丢帧（§3.2 **E2004**）上报成会话事件。
    ///
    /// 丢帧**不**把会话打进 `Error` 态：它只是一条统计事件（§3.11），
    /// 上层据此提示「画面有丢帧」即可。
    fn report_dropped_frames(&mut self) {
        let count = match self.transport.as_mut() {
            Some(t) => t.take_dropped_frames(),
            None => 0,
        };
        if count == 0 {
            return;
        }
        self.emit(SessionEvent::FramesDropped {
            count,
            code: crate::event::FRAME_EXPIRED_CODE,
        });
    }

    /// 定期回报 `Feedback`（§4.1「输入 stream / Feedback」）。
    fn maybe_send_feedback(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        let now = self.now_ms();
        if now.saturating_sub(self.last_stats_ms) < STATS_INTERVAL_MS {
            return;
        }
        self.last_stats_ms = now;
        let feedback = Feedback {
            loss: self.stats.loss_ppm,
            rtt: Duration::from_micros(u64::from(self.stats.rtt_us)),
            recv_rate: self.stats.bitrate_bps,
        };
        self.send_control(ControlMsg::Feedback(feedback));
    }
}

impl SessionRunner for ClientRunner {
    fn apply(&mut self, input: SessionInput) {
        match input {
            SessionInput::Discovering => {
                let _ = self.goto(SessionState::Discovering);
            }
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
                // §4.1：连接建立后先配对、再 `Hello` → 能力协商。
                self.start_handshake();
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
                    reason: format!("会话失败(peer={},code={code})", self.peer.host),
                };
                self.fail(&err);
            }
            SessionInput::Stop => {
                // 发出 `Bye` 后**不立刻丢弃传输**：QUIC 连接的最后一个引用被释放时
                // 会立即关闭连接，尚未发出的 `Bye` 会丢失。传输随句柄释放而回收。
                self.send_control(ControlMsg::Bye);
                let _ = self.goto(SessionState::Closed);
            }
        }
    }

    fn pump(&mut self) {
        self.check_deadlines();
        self.drain_control();
        self.pump_frames();
        self.report_dropped_frames();
        self.maybe_send_feedback();
        self.flush_control();
    }

    fn on_update(&mut self, config: &StreamConfig) {
        // §3.9 `update` 要真的生效：把新配置下发给主机。
        self.send_control(ControlMsg::StreamConfig(config.clone()));
    }

    fn on_stop(&mut self) {
        // 同 `SessionInput::Stop`：保留传输直到会话彻底释放，
        // 否则 QUIC 连接会先于 `Bye` 关闭。
        self.send_control(ControlMsg::Bye);
    }
}

/// 客户端会话（§3.9）。
pub struct ClientSession {
    handle: SessionHandle,
}

impl ClientSession {
    /// 连接主机（§3.9 `ClientSession::connect(peer, cfg) -> SessionHandle`）。
    pub fn connect(peer: PeerAddr, cfg: ClientSessionConfig) -> SessionHandle {
        Self::connect_with(
            peer,
            cfg,
            ClientDeps {
                consumer: None,
                transport: None,
            },
        )
    }

    /// 连接主机并注入解码器与传输（完整版本）。
    pub fn connect_with(
        peer: PeerAddr,
        cfg: ClientSessionConfig,
        deps: ClientDeps,
    ) -> SessionHandle {
        Self::connect_with_control(peer, cfg, deps, ClientControlDeps::default())
    }

    /// 连接主机并注入解码器、传输与控制通道依赖（§4.1 时序）。
    ///
    /// 会话在收到对端的 `HelloAck`（能力协商结果）之后才进入 `Streaming`，
    /// 调用方通过 [`SessionHandle::events`] 观察状态与协商结果。
    pub fn connect_with_control(
        peer: PeerAddr,
        cfg: ClientSessionConfig,
        deps: ClientDeps,
        ctrl_deps: ClientControlDeps,
    ) -> SessionHandle {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let stream_config = cfg.stream.clone();
        let handle = SessionHandle::new(SessionState::Idle, stream_config, tx, rx);
        handle.set_timeouts(SessionTimeouts {
            reconnect_timeout: cfg.reconnect_timeout,
            stall_threshold: cfg.stall_threshold,
        });

        // 传输工厂优先取显式注入的，其次取配置里的。
        let factory = deps.transport.or(cfg.transport);
        let mut transport = None;
        let mut transport_error = None;
        if let Some(mut factory) = factory {
            match factory.connect() {
                Ok(t) => transport = Some(t),
                Err(e) => transport_error = Some(e),
            }
        }

        let ctrl = ControlContext::new(
            ControlRole::Client,
            caps_from_config(&cfg.stream),
            ctrl_deps.pairing,
            ctrl_deps.rate,
            handle.inner().clock(),
        );
        let now = handle.now_ms();
        let connected = transport.is_some();
        let runner = ClientRunner {
            machine: StateMachine::new(),
            peer,
            consumer: deps.consumer,
            transport,
            inner: Arc::downgrade(handle.inner()),
            ctrl,
            session_id: next_session_id(),
            last_data_ms: now,
            reconnect_since_ms: None,
            handshake_started: false,
            hello_sent: false,
            first_frame_seen: false,
            idr_requested: false,
            last_stats_ms: now,
            stats: StatsReport::default(),
        };
        handle.set_runner(Box::new(runner));

        if connected {
            // 客户端主链路：Idle → Connecting → Negotiating →（收到 HelloAck）→ Streaming。
            let _ = handle.apply(SessionInput::Connecting);
            if handle.state() == SessionState::Connecting {
                let _ = handle.apply(SessionInput::Connected);
            }
        } else {
            // 没有传输就无从连接：明确报 `E9002`，而不是静默停在 Idle。
            let err = transport_error.unwrap_or(SessionError::MissingDependency {
                what: "transport".to_string(),
            });
            handle.inner().emit(SessionEvent::Error {
                code: err.code(),
                retryable: err.retryable(),
            });
        }
        handle
    }

    /// 会话句柄。
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::event::ControlChannel;
    use beam_proto::{EncodedFrame, InputEvent};
    use futures::StreamExt;

    /// 可脚本化的假传输：控制消息进队列，发出的控制消息进共享日志。
    struct FakeTransport {
        inbound: std::collections::VecDeque<Result<ControlMsg, SessionError>>,
        sent: std::sync::Arc<std::sync::Mutex<Vec<ControlMsg>>>,
        frames: Vec<EncodedFrame>,
        /// 待上报的丢帧数（§3.2 E2004）。
        dropped: u32,
    }

    impl FakeTransport {
        fn new() -> Self {
            Self {
                inbound: std::collections::VecDeque::new(),
                sent: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                frames: Vec::new(),
                dropped: 0,
            }
        }
    }

    impl ControlChannel for FakeTransport {
        fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
            self.sent.lock().expect("锁").push(msg);
            Ok(())
        }

        fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
            self.inbound.pop_front()
        }
    }

    impl Transport for FakeTransport {
        fn send_frame(&mut self, _f: EncodedFrame) -> Result<(), SessionError> {
            Ok(())
        }

        fn send_input(&mut self, _ev: InputEvent) -> Result<(), SessionError> {
            Ok(())
        }

        fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
            if self.frames.is_empty() {
                None
            } else {
                Some(Ok(self.frames.remove(0)))
            }
        }

        fn take_dropped_frames(&mut self) -> u32 {
            std::mem::take(&mut self.dropped)
        }
    }

    struct FakeFactory {
        transport: Option<Box<dyn Transport>>,
    }

    impl TransportFactory for FakeFactory {
        fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
            Ok(self
                .transport
                .take()
                .unwrap_or_else(|| Box::new(FakeTransport::new())))
        }
    }

    struct FakeConsumer {
        seen: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FrameConsumer for FakeConsumer {
        fn consume(&mut self, _f: EncodedFrame) -> Result<(), SessionError> {
            self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn peer() -> PeerAddr {
        PeerAddr {
            host: "192.168.1.10".into(),
            port: 4827,
            fingerprint: Some("01234567".into()),
        }
    }

    /// 造一个装着指定假传输的客户端会话，并返回 (handle, 假传输)。
    fn session_with(
        transport: FakeTransport,
    ) -> (
        SessionHandle,
        std::sync::Arc<std::sync::Mutex<Vec<ControlMsg>>>,
    ) {
        let sent = std::sync::Arc::clone(&transport.sent);
        let cfg = ClientSessionConfig::default();
        let deps = ClientDeps {
            consumer: None,
            transport: Some(Box::new(FakeFactory {
                transport: Some(Box::new(transport)),
            })),
        };
        let handle = ClientSession::connect_with(peer(), cfg, deps);
        (handle, sent)
    }

    /// 造一个带消费者、装着指定假传输的客户端会话。
    fn session_with_consumer(
        transport: FakeTransport,
        consumer: Box<dyn FrameConsumer>,
    ) -> SessionHandle {
        let cfg = ClientSessionConfig::default();
        let deps = ClientDeps {
            consumer: Some(consumer),
            transport: Some(Box::new(FakeFactory {
                transport: Some(Box::new(transport)),
            })),
        };
        ClientSession::connect_with(peer(), cfg, deps)
    }

    /// 一条标准的 `HelloAck`。
    fn hello_ack() -> ControlMsg {
        ControlMsg::HelloAck {
            agreed: beam_proto::Agreed {
                codec: beam_proto::CodecKind::H264,
                width: 1920,
                height: 1080,
                fps: 60,
                features: 0,
            },
        }
    }

    /// 造一条画面帧。
    fn frame(is_key: bool) -> EncodedFrame {
        EncodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            is_key,
            slices: vec![bytes::Bytes::from_static(b"x")],
        }
    }

    /// 统计发出的控制消息里有多少条 IDR 请求。
    fn idr_request_count(sent: &[ControlMsg]) -> usize {
        sent.iter()
            .filter(|m| matches!(m, ControlMsg::RequestIdr))
            .count()
    }

    #[test]
    fn client_sends_hello_and_waits_for_hello_ack() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        let (handle, sent) = session_with(transport);
        assert_eq!(handle.state(), SessionState::Negotiating);
        let log = sent.lock().expect("锁").clone();
        assert!(
            log.iter().any(|m| matches!(m, ControlMsg::Hello(_))),
            "连接后必须发出 Hello"
        );

        handle.pump().expect("应当能收控制消息");
        assert_eq!(handle.state(), SessionState::Streaming);
        assert!(handle.agreed().is_some());
    }

    #[test]
    fn client_without_transport_reports_missing_dependency() {
        let handle = ClientSession::connect(peer(), ClientSessionConfig::default());
        let mut events = handle.events();
        assert_eq!(
            handle.state(),
            SessionState::Idle,
            "没有传输时不应推进到串流态"
        );
        let mut saw = false;
        while let Some(ev) = futures::executor::block_on(events.next()) {
            if let SessionEvent::Error { code, .. } = ev {
                assert_eq!(code, 9002);
                saw = true;
                break;
            }
        }
        assert!(saw, "应当报告依赖缺失");
    }

    #[test]
    fn client_consumes_frames() {
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        transport.frames.push(EncodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            is_key: true,
            slices: vec![bytes::Bytes::from_static(b"x")],
        });
        let handle = session_with_consumer(
            transport,
            Box::new(FakeConsumer {
                seen: std::sync::Arc::clone(&seen),
            }),
        );
        handle.pump().expect("应当能收控制消息");
        handle.pump().expect("应当能收帧");
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "应当消费到一帧"
        );
    }

    #[test]
    fn stall_threshold_triggers_reconnecting() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        let (handle, _sent) = session_with(transport);
        let clock = std::sync::Arc::new(ManualClock::new());
        handle.set_clock(std::sync::Arc::clone(&clock) as std::sync::Arc<dyn crate::clock::Clock>);
        handle.pump().expect("应当能收控制消息");
        assert_eq!(handle.state(), SessionState::Streaming);

        clock.advance(Duration::from_secs(4));
        handle.pump().expect("应当能推进");
        assert_eq!(
            handle.state(),
            SessionState::Reconnecting,
            "超过 stall_threshold 必须进入重连"
        );
    }

    #[test]
    fn reconnect_timeout_closes_session() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        let (handle, _sent) = session_with(transport);
        let mut events = handle.events();
        let clock = std::sync::Arc::new(ManualClock::new());
        handle.set_clock(std::sync::Arc::clone(&clock) as std::sync::Arc<dyn crate::clock::Clock>);
        handle.pump().expect("应当能收控制消息");

        clock.advance(Duration::from_secs(4));
        handle.pump().expect("应当能推进");
        assert_eq!(handle.state(), SessionState::Reconnecting);
        clock.advance(Duration::from_secs(31));
        handle.pump().expect("应当能推进");
        assert_eq!(
            handle.state(),
            SessionState::Closed,
            "超过 reconnect_timeout 必须关闭"
        );

        let mut saw_timeout = false;
        for _ in 0..8 {
            match futures::executor::block_on(events.next()) {
                Some(SessionEvent::Error { code, .. }) => {
                    if code == 9003 {
                        saw_timeout = true;
                        break;
                    }
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(saw_timeout, "必须报 E9003 会话超时");
    }

    #[test]
    fn update_sends_stream_config_to_peer() {
        let (handle, sent) = session_with(FakeTransport::new());
        let cfg = StreamConfig {
            fps: 144,
            ..StreamConfig::default()
        };
        handle.update(cfg).expect("应当能更新");
        let log = sent.lock().expect("锁").clone();
        assert!(
            log.iter()
                .any(|m| matches!(m, ControlMsg::StreamConfig(c) if c.fps == 144)),
            "update 必须下发 StreamConfig"
        );
    }

    #[test]
    fn stop_sends_bye() {
        let (handle, sent) = session_with(FakeTransport::new());
        handle.stop();
        let log = sent.lock().expect("锁").clone();
        assert!(
            log.iter().any(|m| matches!(m, ControlMsg::Bye)),
            "stop 必须发 Bye"
        );
        assert_eq!(handle.state(), SessionState::Closed);
    }

    /// §4.1「视频 datagram(首帧 IDR)」：首帧不是关键帧 → 请求一次 IDR。
    #[test]
    fn client_requests_idr_once_when_first_frame_is_not_key() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        // 一串「首帧非 IDR」的帧。
        transport.frames.push(frame(false));
        transport.frames.push(frame(false));
        transport.frames.push(frame(false));
        let (handle, sent) = session_with(transport);

        handle.pump().expect("应当能收控制消息与帧");
        assert_eq!(handle.state(), SessionState::Streaming);

        let log = sent.lock().expect("锁").clone();
        assert_eq!(
            idr_request_count(&log),
            1,
            "首帧非 IDR 时必须请求一次 IDR,且不重复请求"
        );
    }

    /// 首帧就是 IDR → 不发 IDR 请求（后续帧也不再触发）。
    #[test]
    fn client_does_not_request_idr_when_first_frame_is_key() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        transport.frames.push(frame(true));
        // 后续帧不是关键帧也不该再请求（首帧策略只看第一帧）。
        transport.frames.push(frame(false));
        let (handle, sent) = session_with(transport);

        handle.pump().expect("应当能收控制消息与帧");
        assert_eq!(handle.state(), SessionState::Streaming);

        let log = sent.lock().expect("锁").clone();
        assert_eq!(idr_request_count(&log), 0, "首帧是 IDR 时不得重复请求 IDR");
    }

    /// §3.2 E2004：丢帧上报成事件,但**不**把会话打进 Error 态。
    #[test]
    fn client_reports_dropped_frames_without_failing() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        transport.dropped = 4;
        let (handle, _sent) = session_with(transport);
        let mut events = handle.events();

        handle.pump().expect("应当能收控制消息");

        let mut saw = false;
        while let Some(ev) = futures::executor::block_on(events.next()) {
            if let SessionEvent::FramesDropped { count, code } = ev {
                assert_eq!(count, 4, "丢帧数必须原样上报");
                assert_eq!(code, crate::event::FRAME_EXPIRED_CODE);
                saw = true;
                break;
            }
        }
        assert!(saw, "必须上报丢帧事件");
        assert_eq!(
            handle.state(),
            SessionState::Streaming,
            "丢帧不是致命错误,会话状态不得改变"
        );
    }
}
