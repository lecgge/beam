//! 本机回环集成测试（§8「假 capture/encoder 在本机回环跑完整会话」的一部分）。
//!
//! 在本机起一对真实的 QUIC 端点（[`beam_net::NetEndpoint::listen`] +
//! [`beam_net::NetEndpoint::connect`]），跑通 **Hello → 能力协商 → HelloAck**，
//! 并断言协商出来的 [`beam_proto::Agreed`] 与预期一致。
//!
//! - 自签证书：`beam_pair::Identity` + `MemoryKeyStore`；
//! - 对端校验：`beam_pair::TrustStore`（双方互相固定指纹）；
//! - 不依赖外网、不依赖真机，可重复运行。

use std::sync::Arc;
use std::time::Duration;

use beam_client::NetTransport;
use beam_net::{NetConfig, NetEndpoint, TlsIdentity};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::{Agreed, CodecKind, ControlMsg, EncodedFrame, StreamConfig};
use beam_session::{
    ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, FramePipeline,
    HostControlDeps, HostDeps, HostSession, HostSessionConfig, SessionEvent, SessionInput,
    SessionState, Transport, TransportFactory, VideoSender,
};
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
    let host_task = tokio::spawn(async move { NetTransport::accept(host_conn).await });
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
            input: None,
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
    let host_task = tokio::spawn(async move { NetTransport::accept(host_conn).await });

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
            input: None,
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
