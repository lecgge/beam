//! 客户端会话（§3.9 `ClientSession::connect(peer, cfg) -> SessionHandle`）。
//!
//! 客户端侧串联：net → decode → render；并把输入事件回传主机。
//! 与主机侧一样，全部依赖通过 trait 注入（§3.9 边界）。

use std::sync::{Arc, Weak};
use std::time::Duration;

use beam_proto::StreamConfig;

use crate::error::SessionError;
use crate::event::{FrameConsumer, SessionEvent, SessionInput, Transport, TransportFactory};
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
pub struct ClientDeps {
    /// 解码 + 上屏。
    pub consumer: Option<Box<dyn FrameConsumer>>,
    /// 传输工厂。
    pub transport: Option<Box<dyn TransportFactory>>,
}

impl Default for ClientDeps {
    fn default() -> Self {
        Self {
            consumer: None,
            transport: None,
        }
    }
}

impl std::fmt::Debug for ClientDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientDeps")
            .field("consumer", &self.consumer.is_some())
            .field("transport", &self.transport.is_some())
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
                    reason: format!("会话失败(peer={},code={code})", self.peer.host),
                };
                self.fail(&err);
            }
            SessionInput::Stop => {
                self.transport = None;
                let _ = self.goto(SessionState::Closed);
            }
        }
    }

    fn pump(&mut self) {
        if self.machine.state() != SessionState::Streaming {
            return;
        }
        let Some(transport) = self.transport.as_mut() else {
            return;
        };
        loop {
            match transport.recv_frame() {
                Some(Ok(frame)) => {
                    if let Some(consumer) = self.consumer.as_mut() {
                        if let Err(e) = consumer.consume(frame) {
                            self.fail(&e);
                            return;
                        }
                    }
                }
                Some(Err(e)) => {
                    self.fail(&e);
                    return;
                }
                None => return,
            }
        }
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

        let connected = transport.is_some();
        let runner = ClientRunner {
            machine: StateMachine::new(),
            peer,
            consumer: deps.consumer,
            transport,
            inner: Arc::downgrade(handle.inner()),
        };
        handle.set_runner(Box::new(runner));

        if connected {
            // 客户端主链路：Idle → Connecting → Negotiating → Streaming。
            let _ = handle.apply(SessionInput::Connecting);
            if handle.state() == SessionState::Connecting {
                let _ = handle.apply(SessionInput::Connected);
            }
            if handle.state() == SessionState::Negotiating {
                let _ = handle.apply(SessionInput::Negotiated(cfg.stream));
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
    use beam_proto::{ControlMsg, EncodedFrame, InputEvent};
    use futures::StreamExt;

    struct FakeTransport {
        frames: Vec<EncodedFrame>,
    }

    impl Transport for FakeTransport {
        fn send_control(&mut self, _msg: ControlMsg) -> Result<(), SessionError> {
            Ok(())
        }
        fn send_frame(&mut self, _f: EncodedFrame) -> Result<(), SessionError> {
            Ok(())
        }
        fn send_input(&mut self, _ev: InputEvent) -> Result<(), SessionError> {
            Ok(())
        }
        fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
            None
        }
        fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
            if self.frames.is_empty() {
                None
            } else {
                Some(Ok(self.frames.remove(0)))
            }
        }
    }

    struct FakeFactory {
        transport: Option<FakeTransport>,
    }

    impl TransportFactory for FakeFactory {
        fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
            Ok(Box::new(
                self.transport
                    .take()
                    .unwrap_or(FakeTransport { frames: Vec::new() }),
            ))
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

    #[test]
    fn client_reaches_streaming_state() {
        let cfg = ClientSessionConfig::default();
        let deps = ClientDeps {
            consumer: None,
            transport: Some(Box::new(FakeFactory { transport: None })),
        };
        let handle = ClientSession::connect_with(peer(), cfg, deps);
        assert_eq!(handle.state(), SessionState::Streaming);
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
        let cfg = ClientSessionConfig::default();
        let deps = ClientDeps {
            consumer: Some(Box::new(FakeConsumer {
                seen: std::sync::Arc::clone(&seen),
            })),
            transport: Some(Box::new(FakeFactory {
                transport: Some(FakeTransport {
                    frames: vec![EncodedFrame {
                        frame_id: 1,
                        pts_us: 0,
                        capture_ts_us: 0,
                        is_key: true,
                        slices: vec![bytes::Bytes::from_static(b"x")],
                    }],
                }),
            })),
        };
        let handle = ClientSession::connect_with(peer(), cfg, deps);
        handle.pump().expect("应当能收帧");
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "应当消费到一帧"
        );
    }
}
