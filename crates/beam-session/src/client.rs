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
//! ## 重连（§4.1「3 s 无帧 → Reconnecting」、产品方案 §3 第 5 步）
//!
//! 断线有两条触发路径，都进 `Reconnecting`：
//!
//! 1. **停滞**：连续 `stall_threshold`（默认 3 s）没收到任何帧 / 控制消息；
//! 2. **传输故障**：控制流或画面流报错（连接真的断了）。
//!
//! 进入 `Reconnecting` 之后：
//!
//! - 传输工厂重建一条**新连接**（§4.1「新连接/迁移」），
//!   在新连接上重发 `Hello` 重新做能力协商，并补一条 `Resume{session_id}`
//!   让主机认回**同一个会话**——**信任已经建立过，不重走 PIN**；
//! - 新连接上收到任何数据即视为恢复：回到 `Streaming` 并请求一次 IDR；
//! - 超过 `reconnect_timeout`（默认 30 s）仍未恢复 → `Closed`（`E9003`）。
//!
//! 重连期间会话**保留最后一帧**：会话层从不清屏，
//! 上层用 [`crate::handle::SessionHandle::reconnecting`] 取
//! [`crate::event::ReconnectingInfo`] 显示「重连中」。
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
    ControlRole, PairingConfig, CLOCK_SYNC_INTERVAL_US, STATS_INTERVAL_MS,
};
use crate::error::SessionError;
use crate::event::{
    FrameConsumer, RateControl, ReconnectingInfo, SessionEvent, SessionInput, Transport,
    TransportFactory,
};
use crate::handle::{SessionHandle, SessionInner, SessionRunner, SessionTimeouts};
use crate::state::{SessionState, StateMachine};
use crate::summary::SessionCounters;

/// 重连尝试间隔（毫秒）。
///
/// §4.1 只要求「30 s 内自动重连」：间隔太短会空转（反复建连），
/// 太长会白白浪费重连窗口。固定 1 s 是个折中：
/// 30 s 窗口内最多尝试 30 次。
pub const RECONNECT_RETRY_INTERVAL_MS: u64 = 1_000;

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
    /// [`ClientSession::connect`] 只接受 `cfg`，因此**传输只能从这里注入**：
    /// 留空时会话会明确报 `E9002`（依赖缺失）并停在 `Idle`，
    /// 因为没有传输就无法建立连接——这比静默挂起更容易定位问题。
    ///
    /// 工厂会被**保留**在会话里：断线重连时用它重建一条新连接
    /// （§4.1「新连接/迁移 + Resume{session_id}」）。
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
    /// 传输工厂：保留下来，重连时用它重建一条**新连接**
    /// （§4.1「新连接/迁移 + Resume{session_id}」）。
    ///
    /// 没有工厂就重建不出传输——此时只能等 `reconnect_timeout` 超时回到列表。
    factory: Option<Box<dyn TransportFactory>>,
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
    /// 上次发 ClockSync 请求的时刻（客户端时钟，微秒；§5「每 5 s 一次」）。
    last_clock_sync_us: u64,
    /// 最近一次时钟同步的结果（§5「客户端通过 ClockSync 求偏移」）。
    clock_sync: Option<crate::clock::ClockSyncState>,
    /// 本端统计。
    stats: StatsReport,
    /// 是否仍需（重新）建立传输：进入 `Reconnecting` 置位，恢复串流才清掉。
    reconnect_pending: bool,
    /// 已尝试重连次数（统计用）。
    reconnect_attempts: u32,
    /// 进入 `Reconnecting` 的次数（统计用）。
    reconnects: u32,
    /// 下次允许重连尝试的时刻（毫秒）。
    next_retry_ms: u64,
    /// 最近一次重建传输的时刻（毫秒）。
    last_rebuild_ms: u64,
    /// 会话开始时刻（毫秒，统计汇总用）。
    started_ms: u64,
    /// 已接收的画面帧数。
    frames_received: u64,
    /// 累计丢帧数。
    dropped_total: u64,
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

    /// 传输层故障（§4.1：断线要能自动重连，而不是直接判死）。
    ///
    /// - 串流中且有传输工厂 → 进 `Reconnecting`，走「重建连接 + `Resume`」；
    /// - 重连中 → 保持重连态，等下一次重试；
    /// - 其余状态（协商中 / 连接中 / 无工厂）→ 进 `Error` 态。
    fn on_transport_error(&mut self, err: &SessionError) {
        match self.machine.state() {
            SessionState::Streaming if self.factory.is_some() => {
                tracing::warn!(reason = %err, "传输故障,进入重连");
                self.enter_reconnecting();
            }
            SessionState::Reconnecting => {
                // 重连期间传输又坏了：保持重连态，等下一次重试。
                tracing::warn!(reason = %err, "重连期间传输再次故障");
                self.reconnect_pending = true;
            }
            _ => self.fail(err),
        }
    }

    fn now_ms(&self) -> u64 {
        self.inner.upgrade().map(|i| i.now_ms()).unwrap_or(0)
    }

    /// 当前单调时间戳（**微秒**，§5「所有时间戳为单调时钟微秒」）。
    fn now_us(&self) -> u64 {
        self.inner.upgrade().map(|i| i.now_us()).unwrap_or(0)
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
        if let Err(e) = self.try_send_control(msg) {
            self.on_transport_error(&e);
        }
    }

    /// 发送一条控制消息并把错误**交回调用方**（不直接判死会话）。
    fn try_send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        match self.transport.as_mut() {
            Some(t) => t.send_control(msg),
            None => Ok(()),
        }
    }

    /// 把发送队列里没写完的字节冲出去（底层缓冲满时需要续写）。
    fn flush_control(&mut self) {
        let result = match self.transport.as_mut() {
            Some(t) => t.flush(),
            None => return,
        };
        if let Err(e) = result {
            self.on_transport_error(&e);
        }
    }

    /// 转到 `Streaming`；已经在串流态时**不重复转移**。
    ///
    /// §3.9 的转移表里没有 `Streaming → Streaming`，
    /// 直接转移会报 `E9001`（非法转移）——重连恢复时很容易撞上
    /// （先由 `on_data` 恢复，再由 `HelloAck` 触发协商完成）。
    fn enter_streaming(&mut self) -> bool {
        if self.machine.state() == SessionState::Streaming {
            return true;
        }
        if self.goto(SessionState::Streaming).is_err() {
            return false;
        }
        self.reconnect_since_ms = None;
        // 恢复串流：不再需要（重新）建立传输。
        self.reconnect_pending = false;
        true
    }

    /// 进入 `Reconnecting`（§4.1「3 s 无帧 → Reconnecting」）。
    ///
    /// 只从 `Streaming` 进入：§3.9 的转移表把 `Reconnecting` 挂在 `Streaming` 之下。
    /// 重连期间**保留最后一帧**：这里不清屏、不动消费者。
    fn enter_reconnecting(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        if self.goto(SessionState::Reconnecting).is_err() {
            return;
        }
        let now = self.now_ms();
        self.reconnect_since_ms = Some(now);
        // 需要重建传输；第一次尝试立即进行（`next_retry_ms = now`）。
        self.reconnect_pending = true;
        self.next_retry_ms = now;
        self.reconnects += 1;
        tracing::info!(session_id = self.session_id, "进入重连");
    }

    /// 尝试重建传输并恢复会话（§4.1「新连接/迁移 + Resume{session_id}」）。
    ///
    /// 信任已经建立过，因此**不重走配对**：
    /// 只在新连接上重发 `Hello` 做一次能力协商，
    /// 再补一条 `Resume{session_id}` 让主机认回同一个会话。
    ///
    /// 重建失败不判死：按 [`RECONNECT_RETRY_INTERVAL_MS`] 间隔重试；
    /// 新连接连上了却没握手成功（收不到 `HelloAck`）时，
    /// 等一个 `stall_threshold` 再重建一条——直到 `reconnect_timeout`
    /// 超时（§4.1「超时回到列表」）。
    fn maybe_reconnect(&mut self) {
        if self.machine.state() != SessionState::Reconnecting || !self.reconnect_pending {
            return;
        }
        let now = self.now_ms();
        if now < self.next_retry_ms {
            return;
        }
        // 刚重建过：先给新连接一个 `stall_threshold` 的时间握手 / 收数据，
        // 别一失败就连着建（§4.1 只要求「30 s 内自动重连」）。
        let threshold = duration_ms(self.timeouts().stall_threshold);
        if self.reconnect_attempts > 0 && now.saturating_sub(self.last_rebuild_ms) < threshold {
            return;
        }
        self.next_retry_ms = now + RECONNECT_RETRY_INTERVAL_MS;
        let Some(factory) = self.factory.as_mut() else {
            // 没有工厂就重建不出传输：只能等重连超时。
            return;
        };
        match factory.connect() {
            Ok(transport) => {
                self.transport = Some(transport);
                self.reconnect_attempts += 1;
                self.last_rebuild_ms = now;
                // 新的一轮串流：首帧策略重新生效（§4.1「首帧 IDR」）。
                self.first_frame_seen = false;
                self.idr_requested = false;
                // 新连接上重新做一次能力协商（`Hello` → `HelloAck`）。
                self.hello_sent = false;
                self.send_hello();
                // §4.1：`Resume{session_id}` 让主机恢复同一个会话（不重新配对）。
                if let Err(e) = self.try_send_control(ControlMsg::Resume {
                    session_id: self.session_id,
                }) {
                    tracing::warn!(reason = %e, "发送 Resume 失败");
                    return;
                }
                tracing::info!(
                    session_id = self.session_id,
                    attempts = self.reconnect_attempts,
                    "已重建传输并请求恢复会话"
                );
            }
            Err(e) => {
                tracing::warn!(session_id = self.session_id, reason = %e, "重建传输失败,稍后重试");
            }
        }
    }

    /// 收到任何数据（帧或控制消息）时调用：刷新时间戳、必要时从重连恢复。
    fn on_data(&mut self) {
        self.last_data_ms = self.now_ms();
        if self.machine.state() == SessionState::Reconnecting {
            // §4.1：30 s 内恢复 → 回到 Streaming，并请求 IDR。
            if self.enter_streaming() {
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
        // §5 的 `ClockSync` 走**专用**通道：客户端发出的是请求（t1=t2=t3=0），
        // 主机回的是应答（t1/t2 非 0）。这里必须先截住应答，
        // 否则它会被当成新的请求再回一条 ⇒ 两端无限乒乓。
        if let ControlMsg::ClockSync { t0, t1, t2, t3 } = msg {
            if t1 != 0 || t2 != 0 || t3 != 0 {
                self.on_clock_sync_reply(t0, t1, t2);
                return;
            }
        }
        let outcome: ControlOutcome = self.ctrl.handle(msg);
        self.apply_outcome(outcome);
    }

    /// 收到主机的 `ClockSync` 应答 ⇒ 估算时钟偏移（§5「客户端通过 ClockSync 求偏移」）。
    fn on_clock_sync_reply(&mut self, t0: u64, t1: u64, t2: u64) {
        let t3 = self.now_us();
        let (offset_us, rtt_us) = crate::clock::estimate_offset(t0, t1, t2, t3);
        self.clock_sync = Some(crate::clock::ClockSyncState {
            offset_us,
            rtt_us,
            measured_at_us: t3,
        });
        // 把偏移告知帧消费者：要算 E2E（§3.13）就需要它把
        // 「主机抓屏时刻」换算到客户端时钟。
        if let Some(consumer) = self.consumer.as_mut() {
            consumer.on_clock_sync(offset_us);
        }
    }

    /// 定期发 `ClockSync` 请求（§3.13「NTP 式四时间戳时钟同步，**每 5 s** 一次」）。
    fn maybe_send_clock_sync(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        let now = self.now_us();
        if now.saturating_sub(self.last_clock_sync_us) < CLOCK_SYNC_INTERVAL_US {
            return;
        }
        self.last_clock_sync_us = now;
        self.send_control(ControlMsg::ClockSync {
            t0: now,
            t1: 0,
            t2: 0,
            t3: 0,
        });
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
            // 协商完成即进入串流态；已经在串流态时只补发事件，
            // 避免 `Streaming → Streaming` 的非法转移（§3.9 转移表）。
            if self.enter_streaming() {
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
                if now.saturating_sub(self.last_data_ms) >= threshold {
                    self.enter_reconnecting();
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
                    // 超时后不再重试：回到列表（§4.1「超时回到列表」）。
                    self.factory = None;
                    self.reconnect_pending = false;
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
                    self.on_transport_error(&e);
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
                    self.frames_received += 1;
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
                    self.on_transport_error(&e);
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
        self.dropped_total += u64::from(count);
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
                self.enter_reconnecting();
            }
            SessionInput::Reconnected => {
                if self.machine.state() == SessionState::Reconnecting && self.enter_streaming() {
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
        self.maybe_reconnect();
        self.drain_control();
        self.pump_frames();
        self.report_dropped_frames();
        self.maybe_send_feedback();
        self.maybe_send_clock_sync();
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

    fn on_clock_changed(&mut self, now_ms: u64) {
        // 换时钟 = 换时间基准：把绝对时间戳重置到新时钟的当前时刻，
        // 否则旧时钟留下的时间戳会让停滞判定永远不成立。
        self.last_data_ms = now_ms;
        self.last_stats_ms = now_ms;
        self.started_ms = now_ms;
        self.reconnect_since_ms = None;
        self.next_retry_ms = now_ms;
        self.last_rebuild_ms = now_ms;
        // 换了时间基准 ⇒ 之前算出的时钟偏移作废（下次同步会重算）。
        self.last_clock_sync_us = 0;
        self.clock_sync = None;
    }

    fn clock_sync_info(&self) -> Option<crate::clock::ClockSyncState> {
        self.clock_sync
    }

    fn reconnecting_info(&self) -> Option<ReconnectingInfo> {
        if self.machine.state() != SessionState::Reconnecting {
            return None;
        }
        let since = self.reconnect_since_ms.unwrap_or_else(|| self.now_ms());
        Some(ReconnectingInfo {
            elapsed_ms: self.now_ms().saturating_sub(since),
            // §4.1 / 产品方案 §3 第 5 步：重连期间保留最后一帧。
            keep_last_frame: true,
        })
    }

    fn counters(&self) -> SessionCounters {
        SessionCounters {
            started_ms: self.started_ms,
            session_id: self.session_id,
            frames: self.frames_received,
            dropped_frames: self.dropped_total,
            reconnects: self.reconnects,
            rtt_us: self.stats.rtt_us,
            loss_ppm: self.stats.loss_ppm,
            fps: self.stats.fps,
            bitrate_bps: self.stats.bitrate_bps,
        }
    }
}

/// 客户端会话（§3.9）。
pub struct ClientSession {
    handle: SessionHandle,
}

impl ClientSession {
    /// 连接主机（§3.9 `ClientSession::connect(peer, cfg) -> SessionHandle`）。
    ///
    /// 本入口只接受 `cfg`，因此**传输必须由 `cfg.transport` 注入**：
    /// 没有传输时会话报 `E9002`（依赖缺失）并停在 `Idle`。
    /// 需要同时注入解码器时用 [`ClientSession::connect_with`]。
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
        // **工厂要留着**：重连时用它重建一条新连接（§4.1「新连接/迁移」）。
        let mut factory = deps.transport.or(cfg.transport);
        let mut transport = None;
        let mut transport_error = None;
        if let Some(f) = factory.as_mut() {
            match f.connect() {
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
            factory,
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
            last_clock_sync_us: 0,
            clock_sync: None,
            stats: StatsReport::default(),
            reconnect_pending: false,
            reconnect_attempts: 0,
            reconnects: 0,
            next_retry_ms: now,
            last_rebuild_ms: now,
            started_ms: now,
            frames_received: 0,
            dropped_total: 0,
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

    /// 统计发出的控制消息里有多少条 `ClockSync`（请求或应答）。
    fn clock_sync_count(sent: &[ControlMsg]) -> usize {
        sent.iter()
            .filter(|m| matches!(m, ControlMsg::ClockSync { .. }))
            .count()
    }

    /// §5「客户端通过 ClockSync 求偏移」：每 5 s 发一次请求。
    #[test]
    fn client_sends_clock_sync_every_five_seconds() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        // 喂若干帧：否则 3 s 的**停滞判定**会先把会话打进重连
        // （真实链路里每 ~16 ms 就有一帧，停滞计时会被不断重置）。
        for _ in 0..8 {
            transport.frames.push(frame(true));
        }
        let (handle, sent) = session_with(transport);
        let clock = std::sync::Arc::new(ManualClock::new());
        handle.set_clock(std::sync::Arc::clone(&clock) as std::sync::Arc<dyn crate::clock::Clock>);
        // 把停滞阈值放宽：本测试**不喂帧**（帧在第一次 pump 里就被一次性抽干），
        // 否则 3 s 的停滞判定会先把会话打进重连（真实链路里每 ~16 ms 一帧，
        // 停滞计时会被不断重置）。
        handle.set_timeouts(crate::SessionTimeouts {
            stall_threshold: Duration::from_secs(60),
            ..crate::SessionTimeouts::default()
        });
        handle.pump().expect("应当能收控制消息");
        assert_eq!(handle.state(), SessionState::Streaming);

        // 逐秒推进（每步都有一帧 ⇒ 不触发停滞判定）。
        for _ in 0..4 {
            clock.advance(Duration::from_secs(1));
            handle.pump().expect("应当能推进");
        }
        assert_eq!(
            clock_sync_count(&sent.lock().expect("锁")),
            0,
            "未到 5 s 不该发 ClockSync"
        );

        // 第 5 秒 ⇒ 发一条请求（t0 = 客户端时钟的微秒）。
        clock.advance(Duration::from_secs(1));
        handle.pump().expect("应当能推进");
        let log = sent.lock().expect("锁").clone();
        assert_eq!(clock_sync_count(&log), 1, "每 5 s 发一次 ClockSync");
        let request = log
            .iter()
            .find_map(|m| match m {
                ControlMsg::ClockSync { t0, t1, t2, t3 } => Some((*t0, *t1, *t2, *t3)),
                _ => None,
            })
            .expect("应当发出 ClockSync 请求");
        assert_eq!(request.0, 5_000_000, "t0 必须是客户端时钟的微秒");
        assert_eq!(
            (request.1, request.2, request.3),
            (0, 0, 0),
            "请求里 t1/t2/t3 为 0"
        );
    }

    /// 收到主机的应答 ⇒ 算出时钟偏移；且**不回**新的 ClockSync（否则两端乒乓）。
    #[test]
    fn client_computes_clock_offset_and_does_not_echo() {
        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        // 主机快 1 s：t1/t2 = 客户端 2.000 s + 1 s = 3.000 s。
        transport.inbound.push_back(Ok(ControlMsg::ClockSync {
            t0: 2_000_000,
            t1: 3_000_000,
            t2: 3_000_000,
            t3: 0,
        }));
        let (handle, sent) = session_with(transport);
        let clock = std::sync::Arc::new(ManualClock::starting_at(2_000));
        handle.set_clock(std::sync::Arc::clone(&clock) as std::sync::Arc<dyn crate::clock::Clock>);

        handle.pump().expect("应当能收 HelloAck");
        assert_eq!(handle.state(), SessionState::Streaming);
        handle.pump().expect("应当能收 ClockSync 应答");

        let sync = handle.clock_sync().expect("应当算出时钟偏移");
        assert_eq!(sync.offset_us, 1_000_000, "偏移应是主机快的那 1 s");
        assert_eq!(sync.rtt_us, 0);
        assert_eq!(sync.measured_at_us, 2_000_000);
        assert_eq!(
            clock_sync_count(&sent.lock().expect("锁")),
            0,
            "客户端不得回新的 ClockSync（否则两端无限乒乓）"
        );
    }
}
