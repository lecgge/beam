//! 主机会话（§3.9 `HostSession::start(cfg, deps) -> SessionHandle`）。
//!
//! 主机侧串联：抓屏 → 编码 → net；并接收客户端输入做注入。
//! 平台能力通过 trait 注入（[`crate::event::FramePipeline`] / [`crate::event::VideoSender`]），
//! 因此本 crate 不依赖任何平台 crate（§3.9 边界）。

use std::sync::{Arc, Weak};
use std::time::Duration;

use beam_proto::StreamConfig;

use crate::error::SessionError;
use crate::event::{FramePipeline, InputSink, SessionEvent, SessionInput, VideoSender};
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
pub struct HostDeps {
    /// 抓屏 → 编码流水线。
    pub pipeline: Box<dyn FramePipeline>,
    /// 视频 / 控制发送器。
    pub sender: Box<dyn VideoSender>,
    /// 输入注入器（可选）。
    pub input: Option<Box<dyn InputSink>>,
}

impl std::fmt::Debug for HostDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostDeps")
            .field("input", &self.input.is_some())
            .finish()
    }
}

/// 主机会话驱动。
pub(crate) struct HostRunner {
    machine: StateMachine,
    deps: HostDeps,
    inner: Weak<SessionInner>,
    pipeline_started: bool,
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
                if self.machine.state() == SessionState::Streaming {
                    let _ = self.goto(SessionState::Reconnecting);
                }
            }
            SessionInput::Reconnected => {
                if self.machine.state() == SessionState::Reconnecting {
                    let _ = self.goto(SessionState::Streaming);
                }
            }
            SessionInput::Failed(code) => {
                let err = SessionError::TransportFailed {
                    reason: format!("会话失败(code={code})"),
                };
                self.fail(&err);
            }
            SessionInput::Stop => {
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
        }
    }
}

/// 主机会话（§3.9）。
pub struct HostSession {
    handle: SessionHandle,
}

impl HostSession {
    /// 启动主机会话（§3.9 `HostSession::start(cfg, deps) -> SessionHandle`）。
    pub fn start(cfg: HostSessionConfig, deps: HostDeps) -> SessionHandle {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let handle = SessionHandle::new(SessionState::Idle, cfg.stream, tx, rx);
        handle.set_timeouts(SessionTimeouts {
            reconnect_timeout: cfg.reconnect_timeout,
            stall_threshold: cfg.stall_threshold,
        });
        let runner = HostRunner {
            machine: StateMachine::new(),
            deps,
            inner: Arc::downgrade(handle.inner()),
            pipeline_started: false,
        };
        handle.set_runner(Box::new(runner));
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
    use crate::event::SessionEvent;
    use beam_proto::{ControlMsg, EncodedFrame};
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakePipeline {
        frames: AtomicUsize,
        started: AtomicUsize,
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
        fn request_idr(&mut self) {}
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

    fn deps() -> HostDeps {
        HostDeps {
            pipeline: Box::new(FakePipeline {
                frames: AtomicUsize::new(0),
                started: AtomicUsize::new(0),
            }),
            sender: Box::new(FakeSender {
                sent: AtomicUsize::new(0),
            }),
            input: None,
        }
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
}
