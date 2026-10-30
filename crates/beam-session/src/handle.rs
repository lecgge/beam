//! 会话句柄（§3.9 `SessionHandle::{events(), update(), stop()}`）。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use beam_proto::{Agreed, StreamConfig};
use futures::channel::mpsc;
use futures::stream::Stream;

use crate::clock::{Clock, SystemClock};
use crate::error::SessionError;
use crate::event::{ReconnectingInfo, SessionEvent, SessionInput};
use crate::state::SessionState;
use crate::summary::{SessionCounters, SessionSummary};

/// 会话超时参数（§3.9：`reconnect_timeout=30s`、`stall_threshold=3s`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTimeouts {
    /// 重连超时。
    pub reconnect_timeout: std::time::Duration,
    /// 停滞阈值。
    pub stall_threshold: std::time::Duration,
}

impl Default for SessionTimeouts {
    fn default() -> Self {
        Self {
            reconnect_timeout: std::time::Duration::from_secs(30),
            stall_threshold: std::time::Duration::from_secs(3),
        }
    }
}

/// 会话驱动（由 host / client 实现）。
pub(crate) trait SessionRunner: Send {
    /// 处理一个会话输入。
    fn apply(&mut self, input: SessionInput);
    /// 推进一步（取帧 / 收帧）。
    fn pump(&mut self);
    /// 配置被更新（§3.9 `SessionHandle::update`）。
    ///
    /// 默认实现什么都不做；客户端据此把新配置下发给主机。
    fn on_update(&mut self, _config: &StreamConfig) {}
    /// 会话被主动结束（§3.9 `SessionHandle::stop`）。
    ///
    /// 默认实现什么都不做；据此发送 `Bye` 并释放传输。
    fn on_stop(&mut self) {}
    /// 会话时钟被替换（§3.9：超时判定必须可用假时钟驱动）。
    ///
    /// 换时钟等于换**时间基准**：运行体必须把「最后一次收到数据」这类
    /// 绝对时间戳重置到新时钟的当前时刻。否则旧时钟留下的时间戳比新时钟大，
    /// `saturating_sub` 恒为 0，停滞判定永远不成立。
    fn on_clock_changed(&mut self, _now_ms: u64) {}
    /// 当前重连信息（§4.1「3 s 无帧 → Reconnecting」）；不在重连态时为 `None`。
    fn reconnecting_info(&self) -> Option<ReconnectingInfo> {
        None
    }
    /// 当前统计计数器（§3.9「统计汇总」）。
    fn counters(&self) -> SessionCounters {
        SessionCounters::default()
    }
}

/// 会话内部状态（句柄与驱动方共享）。
pub(crate) struct SessionInner {
    pub(crate) state: Mutex<SessionState>,
    pub(crate) config: Mutex<StreamConfig>,
    /// 能力协商结果（§3.1 `Agreed`），协商成功后写入。
    pub(crate) agreed: Mutex<Option<Agreed>>,
    pub(crate) timeouts: Mutex<SessionTimeouts>,
    /// 会话时钟（可注入，单测用假时钟推进时间）。
    pub(crate) clock: Mutex<Arc<dyn Clock>>,
    pub(crate) events_tx: mpsc::UnboundedSender<SessionEvent>,
    pub(crate) events_rx: Mutex<Option<mpsc::UnboundedReceiver<SessionEvent>>>,
    pub(crate) stopped: AtomicBool,
    pub(crate) runner: Mutex<Option<Box<dyn SessionRunner>>>,
}

impl std::fmt::Debug for SessionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInner")
            .field("state", &self.state.lock().ok().map(|g| *g))
            .field("stopped", &self.stopped.load(Ordering::Relaxed))
            .finish()
    }
}

impl SessionInner {
    /// 广播一个事件。
    pub(crate) fn emit(&self, event: SessionEvent) {
        let _ = self.events_tx.unbounded_send(event);
    }

    /// 当前时钟毫秒数。
    pub(crate) fn now_ms(&self) -> u64 {
        self.clock.lock().map(|g| g.now_ms()).unwrap_or_default()
    }

    /// 当前超时参数。
    pub(crate) fn timeouts(&self) -> SessionTimeouts {
        self.timeouts.lock().map(|g| *g).unwrap_or_default()
    }

    /// 当前时钟（克隆一份给运行体）。
    pub(crate) fn clock(&self) -> Arc<dyn Clock> {
        self.clock
            .lock()
            .map(|g| Arc::clone(&g))
            .unwrap_or_else(|_| Arc::new(SystemClock))
    }

    /// 当前串流配置。
    pub(crate) fn config(&self) -> StreamConfig {
        self.config.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 覆盖串流配置。
    pub(crate) fn set_config(&self, config: StreamConfig) {
        if let Ok(mut guard) = self.config.lock() {
            *guard = config;
        }
    }

    /// 记录能力协商结果。
    pub(crate) fn set_agreed(&self, agreed: Option<Agreed>) {
        if let Ok(mut guard) = self.agreed.lock() {
            *guard = agreed;
        }
    }
}

/// 会话句柄（§3.9）。
#[derive(Debug, Clone)]
pub struct SessionHandle {
    inner: Arc<SessionInner>,
}

impl SessionHandle {
    pub(crate) fn new(
        state: SessionState,
        config: StreamConfig,
        events_tx: mpsc::UnboundedSender<SessionEvent>,
        events_rx: mpsc::UnboundedReceiver<SessionEvent>,
    ) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                state: Mutex::new(state),
                config: Mutex::new(config),
                agreed: Mutex::new(None),
                timeouts: Mutex::new(SessionTimeouts::default()),
                clock: Mutex::new(Arc::new(SystemClock)),
                events_tx,
                events_rx: Mutex::new(Some(events_rx)),
                stopped: AtomicBool::new(false),
                runner: Mutex::new(None),
            }),
        }
    }

    /// 注入时钟（§3.9：超时判定必须可用假时钟驱动）。
    ///
    /// 单测注入 [`crate::clock::ManualClock`] 后即可推进时间，
    /// 覆盖「3 s 停滞 → Reconnecting」「30 s 超时 → Closed」两条判定。
    ///
    /// 换时钟会**重置时间基准**：运行体里「最后一次收到数据」等绝对时间戳
    /// 会被重置到新时钟的当前时刻（否则旧时钟的时间戳会让停滞判定永远不成立）。
    pub fn set_clock(&self, clock: Arc<dyn Clock>) {
        if let Ok(mut guard) = self.inner.clock.lock() {
            *guard = clock;
        }
        let now = self.now_ms();
        if let Ok(mut guard) = self.inner.runner.lock() {
            if let Some(runner) = guard.as_mut() {
                runner.on_clock_changed(now);
            }
        }
    }

    /// 当前时钟毫秒数。
    pub fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }

    /// 能力协商结果（§3.1 `Agreed`）；尚未协商时为 `None`。
    pub fn agreed(&self) -> Option<Agreed> {
        self.inner.agreed.lock().ok().and_then(|g| g.clone())
    }

    /// 设置超时参数。
    pub(crate) fn set_timeouts(&self, timeouts: SessionTimeouts) {
        if let Ok(mut guard) = self.inner.timeouts.lock() {
            *guard = timeouts;
        }
    }

    /// 超时参数。
    pub fn timeouts(&self) -> SessionTimeouts {
        self.inner.timeouts.lock().map(|g| *g).unwrap_or_default()
    }

    /// 注入会话驱动。
    pub(crate) fn set_runner(&self, runner: Box<dyn SessionRunner>) {
        if let Ok(mut guard) = self.inner.runner.lock() {
            *guard = Some(runner);
        }
    }

    /// 投递一个会话输入（驱动状态机）。
    pub fn apply(&self, input: SessionInput) -> Result<(), SessionError> {
        if self.is_stopped() {
            return Err(SessionError::SessionClosed);
        }
        let mut guard = self
            .inner
            .runner
            .lock()
            .map_err(|_| SessionError::SessionClosed)?;
        if let Some(runner) = guard.as_mut() {
            runner.apply(input);
        }
        Ok(())
    }

    /// 推进一步（取帧 / 收帧）。
    pub fn pump(&self) -> Result<(), SessionError> {
        if self.is_stopped() {
            return Err(SessionError::SessionClosed);
        }
        let mut guard = self
            .inner
            .runner
            .lock()
            .map_err(|_| SessionError::SessionClosed)?;
        if let Some(runner) = guard.as_mut() {
            runner.pump();
        }
        Ok(())
    }

    /// 订阅会话事件（§3.9 `events()`）。
    ///
    /// 只能取一次；重复调用返回一个已关闭的空流。
    pub fn events(&self) -> SessionEventStream {
        let rx = self
            .inner
            .events_rx
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        SessionEventStream {
            rx: rx.unwrap_or_else(|| {
                let (_tx, rx) = mpsc::unbounded();
                rx
            }),
        }
    }

    /// 更新串流配置（§3.9 `update(StreamConfig)`）。
    ///
    /// 配置写入后会通知运行体，由它把变更下发到对端
    /// （客户端发 `ControlMsg::StreamConfig`），因此这条调用**真的生效**。
    pub fn update(&self, config: StreamConfig) -> Result<(), SessionError> {
        if self.inner.stopped.load(Ordering::Relaxed) {
            return Err(SessionError::SessionClosed);
        }
        {
            let mut guard = self
                .inner
                .config
                .lock()
                .map_err(|_| SessionError::SessionClosed)?;
            *guard = config.clone();
        }
        // 注意：先释放 config 锁再进 runner，避免运行体回读配置时死锁。
        let mut guard = self
            .inner
            .runner
            .lock()
            .map_err(|_| SessionError::SessionClosed)?;
        if let Some(runner) = guard.as_mut() {
            runner.on_update(&config);
        }
        Ok(())
    }

    /// 当前串流配置。
    pub fn config(&self) -> StreamConfig {
        self.inner
            .config
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// 当前状态。
    pub fn state(&self) -> SessionState {
        self.inner
            .state
            .lock()
            .map(|g| *g)
            .unwrap_or(SessionState::Closed)
    }

    /// 主动结束会话（§3.9 `stop()`）。
    ///
    /// 先通知运行体收尾（发 `Bye`、释放传输），再置终态并发事件。
    pub fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut guard) = self.inner.runner.lock() {
            if let Some(runner) = guard.as_mut() {
                runner.on_stop();
            }
        }
        if let Ok(mut guard) = self.inner.state.lock() {
            *guard = SessionState::Closed;
        }
        self.inner.emit(SessionEvent::Closed);
    }

    /// 是否已停止。
    pub fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::Relaxed)
    }

    /// 会话统计汇总（§3.9「统计汇总」）。
    ///
    /// 会话结束后依然可以查询：计数器由运行体持有，
    /// 随句柄一起活到句柄被丢弃为止，不需要在会话结束时额外收尾上报。
    pub fn summary(&self) -> SessionSummary {
        let state = self.state();
        let now = self.now_ms();
        let guard = self.inner.runner.lock().ok();
        let counters = match guard.as_ref().and_then(|runner| runner.as_ref()) {
            Some(runner) => runner.counters(),
            None => SessionCounters::default(),
        };
        counters.snapshot(now, state)
    }

    /// 当前的重连信息；不在重连态时为 `None`。
    ///
    /// 上层据此显示「重连中」并**保留最后一帧**
    /// （§4.1「3 s 无帧 → Reconnecting」、产品方案 §3 第 5 步）。
    pub fn reconnecting(&self) -> Option<ReconnectingInfo> {
        let guard = self.inner.runner.lock().ok()?;
        let runner = guard.as_ref()?;
        runner.reconnecting_info()
    }

    /// 当前是否只剩**这一份**句柄（其他持有者都已经放开）。
    ///
    /// 后台线程用它感知「调用方是否已经丢弃了会话」：
    /// 会话句柄被放开后，会话与注入的回调对象都该被释放，
    /// 后台线程因此可以自行退出，不必等会话超时。
    pub fn is_last_handle(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    pub(crate) fn inner(&self) -> &Arc<SessionInner> {
        &self.inner
    }
}

/// 会话事件流。
#[derive(Debug)]
pub struct SessionEventStream {
    rx: mpsc::UnboundedReceiver<SessionEvent>,
}

impl Stream for SessionEventStream {
    type Item = SessionEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::SessionEvent;
    use futures::StreamExt;

    fn handle() -> SessionHandle {
        let (tx, rx) = mpsc::unbounded();
        SessionHandle::new(SessionState::Idle, StreamConfig::default(), tx, rx)
    }

    #[test]
    fn events_can_be_received() {
        let h = handle();
        let mut stream = h.events();
        h.inner().emit(SessionEvent::Paired {
            fingerprint: "abc".into(),
        });
        let ev = futures::executor::block_on(stream.next()).expect("应当有事件");
        assert!(matches!(ev, SessionEvent::Paired { .. }));
    }

    #[test]
    fn second_events_call_returns_closed_stream() {
        let h = handle();
        let _first = h.events();
        let mut second = h.events();
        assert!(futures::executor::block_on(second.next()).is_none());
    }

    #[test]
    fn update_and_stop() {
        let h = handle();
        let cfg = StreamConfig {
            fps: 144,
            ..StreamConfig::default()
        };
        h.update(cfg.clone()).expect("应当能更新");
        assert_eq!(h.config().fps, 144);
        h.stop();
        assert!(h.is_stopped());
        assert_eq!(h.state(), SessionState::Closed);
        assert_eq!(h.update(cfg).unwrap_err().code(), 9004);
    }
}
