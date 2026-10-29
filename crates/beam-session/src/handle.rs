//! 会话句柄（§3.9 `SessionHandle::{events(), update(), stop()}`）。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use beam_proto::StreamConfig;
use futures::channel::mpsc;
use futures::stream::Stream;

use crate::error::SessionError;
use crate::event::{SessionEvent, SessionInput};
use crate::state::SessionState;

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
}

/// 会话内部状态（句柄与驱动方共享）。
pub(crate) struct SessionInner {
    pub(crate) state: Mutex<SessionState>,
    pub(crate) config: Mutex<StreamConfig>,
    pub(crate) timeouts: Mutex<SessionTimeouts>,
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
                timeouts: Mutex::new(SessionTimeouts::default()),
                events_tx,
                events_rx: Mutex::new(Some(events_rx)),
                stopped: AtomicBool::new(false),
                runner: Mutex::new(None),
            }),
        }
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
    pub fn update(&self, config: StreamConfig) -> Result<(), SessionError> {
        if self.inner.stopped.load(Ordering::Relaxed) {
            return Err(SessionError::SessionClosed);
        }
        let mut guard = self
            .inner
            .config
            .lock()
            .map_err(|_| SessionError::SessionClosed)?;
        *guard = config;
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
    pub fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::SeqCst) {
            return;
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
        let mut cfg = StreamConfig::default();
        cfg.fps = 144;
        h.update(cfg.clone()).expect("应当能更新");
        assert_eq!(h.config().fps, 144);
        h.stop();
        assert!(h.is_stopped());
        assert_eq!(h.state(), SessionState::Closed);
        assert_eq!(h.update(cfg).unwrap_err().code(), 9004);
    }
}
