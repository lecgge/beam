//! 本机回环集成测试（§8「假 capture/encoder 在本机回环跑完整会话」的一部分）。
//!
//! 在本机起一对真实的 QUIC 端点（[`beam_net::NetEndpoint::listen`] +
//! [`beam_net::NetEndpoint::connect`]），跑通 **Hello → 能力协商 → HelloAck**，
//! 并断言协商出来的 [`beam_proto::Agreed`] 与预期一致。
//!
//! - 自签证书：`beam_pair::Identity` + `MemoryKeyStore`；
//! - 对端校验：`beam_pair::TrustStore`（双方互相固定指纹）；
//! - 不依赖外网、不依赖真机，可重复运行。
//!
//! 主机侧的 QUIC 接受端已经下沉到 `beam-host`（[`beam_host::HostTransport`]），
//! 本文件同时覆盖三条通道：
//! 控制（双向可靠流）、画面（datagram）、输入（单向可靠流）。

use std::sync::Arc;
use std::time::Duration;

use beam_client::NetTransport;
use beam_host::{HostInput, HostTransport};
use beam_net::{NetConfig, NetEndpoint, TlsIdentity, VideoSender as NetVideoSender};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::{
    Agreed, CodecKind, ControlMsg, EncodedFrame, InputEvent, KeyAction, StreamConfig,
};
use beam_session::{
    ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, ControlChannel,
    FramePipeline, HostControlDeps, HostDeps, HostSession, HostSessionConfig, SessionEvent,
    SessionInput, SessionState, Transport, TransportFactory, VideoSender,
};
use bytes::Bytes;
use futures::StreamExt;

/// 客户端能力：1080p60 / H.264 / 音频。
fn client_stream() -> StreamConfig {
    StreamConfig {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_bps: 20_000_000,
        codec: CodecKind::H264,
        audio_enabled: true,
        features: 0,
    }
}

/// 主机能力：720p30 / H.264 / 音频。
fn host_stream() -> StreamConfig {
    StreamConfig {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_bps: 12_000_000,
        codec: CodecKind::H264,
        audio_enabled: true,
        features: 0,
    }
}

/// 期望的协商结果：取双方分辨率 / 帧率的较小值，编解码器取公共最大值。
fn expected_agreed() -> Agreed {
    Agreed {
        codec: CodecKind::H264,
        width: 1280,
        height: 720,
        fps: 30,
        features: beam_proto::caps::feature::AUDIO,
    }
}

fn net_config(identity: &Identity) -> NetConfig {
    NetConfig {
        identity: Some(TlsIdentity {
            cert_chain: identity.certificate_chain(),
            key: identity.private_key(),
        }),
        listen_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
        bind_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
        server_name: "beam.local".to_string(),
        ..NetConfig::default()
    }
}

/// 只信任指定指纹的信任库。
fn trust_store(peer: &Identity) -> TrustStore {
    let mut store = TrustStore::new();
    store.upsert(TrustedPeer::new(peer.fingerprint(), "peer", "windows", 0));
    store
}

/// 已经建好的传输（免去同步工厂里的阻塞连接）。
struct FixedFactory {
    transport: Option<Box<dyn Transport>>,
}

impl TransportFactory for FixedFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, beam_session::SessionError> {
        self.transport
            .take()
            .ok_or_else(|| beam_session::SessionError::MissingDependency {
                what: "transport".to_string(),
            })
    }
}

/// 主机侧的空流水线（回环测试只验证控制面）。
struct NoopPipeline;

impl FramePipeline for NoopPipeline {
    fn start(&mut self) -> Result<(), beam_session::SessionError> {
        Ok(())
    }
    fn poll(&mut self) -> Option<EncodedFrame> {
        None
    }
    fn request_idr(&mut self) {}
    fn stop(&mut self) {}
}

/// 主机侧的空发送器（画面走 datagram，这里不关心）。
struct NoopSender;

impl VideoSender for NoopSender {
    fn send_frame(&mut self, _f: EncodedFrame) -> Result<(), beam_session::SessionError> {
        Ok(())
    }
    fn send_control(&mut self, _msg: ControlMsg) -> Result<(), beam_session::SessionError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_hello_negotiate_hello_ack() {
    // 1. 双方身份与信任库。
    let host_identity = Identity::generate().expect("应当能生成主机身份");
    let client_identity = Identity::generate().expect("应当能生成客户端身份");
    let host_trust = Arc::new(trust_store(&client_identity));
    let client_trust = Arc::new(trust_store(&host_identity));

    // 2. 真实 QUIC 端点：主机监听，客户端连接。
    let host_endpoint = NetEndpoint::listen(net_config(&host_identity), host_trust)
        .await
        .expect("主机应当能监听");
    let addr = host_endpoint.local_addr().expect("应当能读到地址");

    let (host_conn, client_conn) = tokio::join!(
        host_endpoint.accept(),
        NetEndpoint::connect(addr, net_config(&client_identity), client_trust)
    );
    let host_conn = host_conn.expect("主机应当接受到连接");
    let client_conn = client_conn.expect("客户端应当连接成功");

    // 3. 客户端开控制双向流 + 输入单向流。
    let client_transport = NetTransport::connect(client_conn)
        .await
        .expect("客户端应当能打开控制流");

    // 主机侧接受控制流：QUIC 的流要等对端发数据才可见，
    // 因此这一步要等客户端发出 Hello 之后才会返回。
    let host_task = tokio::spawn(async move { HostTransport::accept(host_conn).await });
    let mut client_transport = Some(client_transport);

    // 4. 客户端会话：建立后立刻发 Hello（§4.1 时序）。
    let client_cfg = ClientSessionConfig {
        stream: client_stream(),
        ..ClientSessionConfig::default()
    };
    let client_handle = ClientSession::connect_with_control(
        beam_session::PeerAddr {
            host: "127.0.0.1".into(),
            port: addr.port(),
            fingerprint: Some(host_identity.fingerprint().to_hex()),
        },
        client_cfg,
        ClientDeps {
            consumer: None,
            transport: Some(Box::new(FixedFactory {
                transport: client_transport
                    .take()
                    .map(|t| Box::new(t) as Box<dyn Transport>),
            })),
        },
        ClientControlDeps::default(),
    );
    let mut client_events = client_handle.events();
    assert_eq!(
        client_handle.state(),
        SessionState::Negotiating,
        "发出 Hello 后应停在协商态，等待 HelloAck"
    );

    // 5. 主机接受控制流并开会话。
    let host_transport = host_task
        .await
        .expect("任务不应当 panic")
        .expect("主机应当能接受控制流");
    let host_handle = HostSession::start_with_control(
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        HostDeps {
            pipeline: Box::new(NoopPipeline),
            sender: Box::new(NoopSender),
        },
        Some(HostControlDeps {
            control: Box::new(host_transport),
            pairing: beam_session::PairingConfig::default(),
            rate: None,
        }),
    );
    let mut host_events = host_handle.events();
    host_handle
        .apply(SessionInput::Connecting)
        .expect("应当能进入连接态");
    host_handle
        .apply(SessionInput::Connected)
        .expect("应当能进入协商态");

    // 6. 推进两端，直到客户端收到 HelloAck。
    for _ in 0..400 {
        client_handle.pump().expect("客户端推进");
        host_handle.pump().expect("主机推进");
        if client_handle.state() == SessionState::Streaming {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // 7. 断言：协商结果两端一致，且与预期相符。
    assert_eq!(
        client_handle.state(),
        SessionState::Streaming,
        "客户端必须收到 HelloAck 后才进入串流态"
    );
    assert_eq!(
        client_handle.agreed(),
        Some(expected_agreed()),
        "客户端协商到的 Agreed 必须与预期一致"
    );
    assert_eq!(
        host_handle.agreed(),
        Some(expected_agreed()),
        "主机协商到的 Agreed 必须与预期一致"
    );
    println!(
        "[回环] client_state={} host_state={} agreed={:?}",
        client_handle.state(),
        host_handle.state(),
        client_handle.agreed()
    );

    // 8. 断言：客户端收到 `Negotiated` 事件，且配置由 Agreed 推导而来。
    let mut negotiated = None;
    while let Some(event) = client_events.next().await {
        if let SessionEvent::Negotiated { config } = event {
            negotiated = Some(config);
            break;
        }
    }
    let config = negotiated.expect("客户端必须收到 Negotiated 事件");
    assert_eq!(config.width, 1280);
    assert_eq!(config.height, 720);
    assert_eq!(config.fps, 30);
    assert_eq!(config.codec, CodecKind::H264);
    assert!(config.audio_enabled);

    // 9. 断言：主机也发出了 `Negotiated` 事件。
    let mut host_negotiated = None;
    while let Some(event) = host_events.next().await {
        if let SessionEvent::Negotiated { config } = event {
            host_negotiated = Some(config);
            break;
        }
    }
    assert!(host_negotiated.is_some(), "主机必须发出 Negotiated 事件");
    println!(
        "[回环] client_negotiated={:?} host_negotiated={:?}",
        config, host_negotiated
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_client_and_host_can_talk_over_control_stream() {
    // 复用上面的建链流程，额外验证：客户端发 `Bye` → 主机关闭会话。
    let host_identity = Identity::generate().expect("应当能生成主机身份");
    let client_identity = Identity::generate().expect("应当能生成客户端身份");
    let host_trust = Arc::new(trust_store(&client_identity));
    let client_trust = Arc::new(trust_store(&host_identity));

    let host_endpoint = NetEndpoint::listen(net_config(&host_identity), host_trust)
        .await
        .expect("主机应当能监听");
    let addr = host_endpoint.local_addr().expect("应当能读到地址");
    let (host_conn, client_conn) = tokio::join!(
        host_endpoint.accept(),
        NetEndpoint::connect(addr, net_config(&client_identity), client_trust)
    );
    let host_conn = host_conn.expect("主机应当接受到连接");
    let client_conn = client_conn.expect("客户端应当连接成功");

    let client_transport = NetTransport::connect(client_conn)
        .await
        .expect("客户端应当能打开控制流");
    let host_task = tokio::spawn(async move { HostTransport::accept(host_conn).await });

    let client_cfg = ClientSessionConfig {
        stream: client_stream(),
        ..ClientSessionConfig::default()
    };
    let mut client_transport = Some(client_transport);
    let client_handle = ClientSession::connect_with_control(
        beam_session::PeerAddr {
            host: "127.0.0.1".into(),
            port: addr.port(),
            fingerprint: None,
        },
        client_cfg,
        ClientDeps {
            consumer: None,
            transport: Some(Box::new(FixedFactory {
                transport: client_transport
                    .take()
                    .map(|t| Box::new(t) as Box<dyn Transport>),
            })),
        },
        ClientControlDeps::default(),
    );

    let host_transport = host_task
        .await
        .expect("任务不应当 panic")
        .expect("主机应当能接受控制流");
    let host_handle = HostSession::start_with_control(
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        HostDeps {
            pipeline: Box::new(NoopPipeline),
            sender: Box::new(NoopSender),
        },
        Some(HostControlDeps {
            control: Box::new(host_transport),
            pairing: beam_session::PairingConfig::default(),
            rate: None,
        }),
    );
    host_handle
        .apply(SessionInput::Connecting)
        .expect("应当能进入连接态");
    host_handle
        .apply(SessionInput::Connected)
        .expect("应当能进入协商态");

    for _ in 0..400 {
        client_handle.pump().expect("客户端推进");
        host_handle.pump().expect("主机推进");
        if client_handle.state() == SessionState::Streaming {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(client_handle.state(), SessionState::Streaming);

    // 客户端主动结束 → 主机侧收到 `Bye` 后关闭会话。
    client_handle.stop();
    for _ in 0..200 {
        host_handle.pump().expect("主机推进");
        if host_handle.state() == SessionState::Closed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        host_handle.state(),
        SessionState::Closed,
        "收到 Bye 后主机必须关闭会话"
    );
}

/// 建立一对真实 QUIC 端点，返回（主机连接，客户端连接）。
async fn quic_pair() -> (beam_net::Conn, beam_net::Conn) {
    let host_identity = Identity::generate().expect("应当能生成主机身份");
    let client_identity = Identity::generate().expect("应当能生成客户端身份");
    let host_trust = Arc::new(trust_store(&client_identity));
    let client_trust = Arc::new(trust_store(&host_identity));

    let host_endpoint = NetEndpoint::listen(net_config(&host_identity), host_trust)
        .await
        .expect("主机应当能监听");
    let addr = host_endpoint.local_addr().expect("应当能读到地址");
    let (host_conn, client_conn) = tokio::join!(
        host_endpoint.accept(),
        NetEndpoint::connect(addr, net_config(&client_identity), client_trust)
    );
    (
        host_conn.expect("主机应当接受到连接"),
        client_conn.expect("客户端应当连接成功"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_video_frames_flow_over_datagram() {
    // §5 画面通道:主机 datagram → 客户端 `recv_frame`。
    let (host_conn, client_conn) = quic_pair().await;

    // 客户端建链(开控制流 + 输入流),并持久持有一个 `VideoStream`。
    let mut client_transport = NetTransport::connect(client_conn)
        .await
        .expect("客户端应当能打开控制流");

    // 往控制流写一条消息,让主机侧的 `accept_bi` 返回(QUIC 的流要收到数据才可见)。
    client_transport
        .send_control(ControlMsg::RequestIdr)
        .expect("应当能写控制消息");

    let host_transport = HostTransport::accept(host_conn)
        .await
        .expect("主机应当能接受控制流");

    // 主机发一帧画面(datagram)。
    let frame = EncodedFrame {
        frame_id: 7,
        pts_us: 1234,
        capture_ts_us: 1200,
        is_key: true,
        slices: vec![Bytes::from_static(b"fake-nal-payload")],
    };
    host_transport
        .send_frame(frame.clone())
        .expect("主机应当能发出画面");

    // 客户端轮询取帧(非阻塞,只消费已经到达的数据报)。
    let mut got: Option<EncodedFrame> = None;
    for _ in 0..400 {
        match client_transport.recv_frame() {
            Some(Ok(f)) => {
                got = Some(f);
                break;
            }
            Some(Err(e)) => panic!("取帧失败: {e}"),
            None => tokio::time::sleep(Duration::from_millis(5)).await,
        }
    }
    let got = got.expect("客户端必须收到主机发出的画面帧");
    assert_eq!(got.frame_id, 7);
    assert_eq!(got.capture_ts_us, 1200);
    // §5 的 16 字节帧头只有 `capture_ts` 一个时间戳字段，没有独立的 `pts`，
    // 因此接收端把 `pts_us` 取成 `capture_ts`（beam-net 的重组器就是这么做的）。
    assert_eq!(got.pts_us, got.capture_ts_us);
    assert!(got.is_key);
    assert_eq!(got.to_payload().as_ref(), b"fake-nal-payload");

    // 帧通道统计:交付了 1 帧,且没有过期帧。
    let stats = client_transport.frame_stats();
    assert_eq!(stats.frames_delivered, 1);
    assert_eq!(stats.frames_expired, 0);
    println!("[回环] 帧通道 stats={stats:?}");
}

/// 主机侧只产出一帧画面的流水线(模拟 capture → encode)。
struct OneFramePipeline {
    sent: bool,
}

impl FramePipeline for OneFramePipeline {
    fn start(&mut self) -> Result<(), beam_session::SessionError> {
        Ok(())
    }
    fn poll(&mut self) -> Option<EncodedFrame> {
        if self.sent {
            return None;
        }
        self.sent = true;
        Some(EncodedFrame {
            frame_id: 99,
            pts_us: 5000,
            capture_ts_us: 5000,
            is_key: true,
            slices: vec![Bytes::from_static(b"session-frame")],
        })
    }
    fn request_idr(&mut self) {}
    fn stop(&mut self) {}
}

/// 客户端侧记录收到的画面帧。
#[derive(Clone, Default)]
struct RecordingConsumer {
    received: Arc<std::sync::Mutex<Vec<EncodedFrame>>>,
}

impl beam_session::FrameConsumer for RecordingConsumer {
    fn consume(&mut self, f: EncodedFrame) -> Result<(), beam_session::SessionError> {
        self.received.lock().expect("锁").push(f);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_session_streams_frame_from_host_to_client() {
    // 端到端:主机流水线 → 主机会话 → datagram → 客户端会话 → 消费者。
    let (host_conn, client_conn) = quic_pair().await;

    let client_transport = NetTransport::connect(client_conn)
        .await
        .expect("客户端应当能打开控制流");
    let host_task = tokio::spawn(async move { HostTransport::accept(host_conn).await });
    let mut client_transport = Some(client_transport);

    let consumer = RecordingConsumer::default();
    let received = consumer.received.clone();
    let client_handle = ClientSession::connect_with_control(
        beam_session::PeerAddr {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: None,
        },
        ClientSessionConfig {
            stream: client_stream(),
            ..ClientSessionConfig::default()
        },
        ClientDeps {
            consumer: Some(Box::new(consumer)),
            transport: Some(Box::new(FixedFactory {
                transport: client_transport
                    .take()
                    .map(|t| Box::new(t) as Box<dyn Transport>),
            })),
        },
        ClientControlDeps::default(),
    );

    let host_transport = host_task
        .await
        .expect("任务不应当 panic")
        .expect("主机应当能接受控制流");
    // 主机侧的画面发送器:真实走 datagram。
    let host_sender = beam_host::NetVideoSender::new(host_transport.conn().clone());
    let host_handle = HostSession::start_with_control(
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        HostDeps {
            pipeline: Box::new(OneFramePipeline { sent: false }),
            sender: Box::new(host_sender),
        },
        Some(HostControlDeps {
            control: Box::new(host_transport),
            pairing: beam_session::PairingConfig::default(),
            rate: None,
        }),
    );
    host_handle
        .apply(SessionInput::Connecting)
        .expect("应当能进入连接态");
    host_handle
        .apply(SessionInput::Connected)
        .expect("应当能进入协商态");

    // 两端推进:协商完成后主机发帧,客户端收帧。
    let mut got = None;
    for _ in 0..400 {
        client_handle.pump().expect("客户端推进");
        host_handle.pump().expect("主机推进");
        let frames = received.lock().expect("锁").clone();
        if let Some(frame) = frames.into_iter().next() {
            got = Some(frame);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let frame = got.expect("客户端会话必须收到主机发出的画面帧");
    assert_eq!(frame.frame_id, 99);
    assert_eq!(frame.to_payload().as_ref(), b"session-frame");
    println!(
        "[回环] 会话帧通道 frame_id={} payload={:?}",
        frame.frame_id,
        String::from_utf8_lossy(&frame.to_payload())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_input_events_flow_over_uni_stream() {
    // §5 输入通道:客户端单向流 → 主机 `accept_uni`。
    let (host_conn, client_conn) = quic_pair().await;

    let mut client_transport = NetTransport::connect(client_conn)
        .await
        .expect("客户端应当能打开控制流");

    // 客户端发一个输入事件(单向可靠流)。
    let event = InputEvent::Key {
        code: 42,
        action: KeyAction::Down,
    };
    client_transport
        .send_input(event.clone())
        .expect("客户端应当能发出输入事件");

    // 主机侧接受输入流并取事件。
    let mut host_input = HostInput::accept(&host_conn)
        .await
        .expect("主机应当能接受输入流");
    let received = host_input
        .next_event()
        .await
        .expect("应当收到输入事件")
        .expect("输入事件应当能解码");
    assert_eq!(received, event);
    println!("[回环] 输入通道 event={received:?}");
}
