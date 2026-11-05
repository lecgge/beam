//! 本机回环集成测试：真实 QUIC 上跑会话层（§3.9 / §4.1 / §5）。
//!
//! ## 覆盖什么
//!
//! - **控制面**：客户端 `Hello` → 主机 `HelloAck` → 两端进入 `Streaming`；
//! - **画面面**：主机 datagram → 客户端会话 → 消费者（§5 视频 datagram）；
//! - **首帧策略**（§4.1「视频 datagram(首帧 IDR)」）：首帧不是关键帧时，
//!   客户端发**一次** `RequestIdr`，主机因此补一个 IDR。
//!
//! ## 怎么做到「真实 QUIC」
//!
//! - 自签证书：`beam_pair::Identity` + `TrustStore`（双方互相固定指纹）；
//! - 端点：`beam_net::NetEndpoint::listen` / `connect`（真实 QUIC + TLS1.3）；
//! - 控制通道：真实 QUIC 双向流（§5 控制 stream 0），分帧用
//!   [`beam_net::framing`]（线上格式的**唯一真值来源**）；
//! - 画面：真实 QUIC datagram（§5）。
//!
//! 不依赖外网、不依赖真机，可重复运行。

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use beam_net::{FrameDecoder, NetConfig, NetEndpoint, TlsIdentity, VideoSender as NetVideoSender};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::{ControlMsg, EncodedFrame, InputEvent, StreamConfig};
use beam_session::{
    ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, ControlChannel,
    FrameConsumer, FramePipeline, HostControlDeps, HostDeps, HostSession, HostSessionConfig,
    PairingConfig, PeerAddr, SessionError, SessionEvent, SessionInput, SessionState, Transport,
    TransportFactory, VideoSender,
};
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 客户端能力：1080p60 / H.264。
fn client_stream() -> StreamConfig {
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

/// 主机能力：720p30 / H.264。
fn host_stream() -> StreamConfig {
    StreamConfig {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_bps: 12_000_000,
        codec: beam_proto::CodecKind::H264,
        audio_enabled: false,
        features: 0,
    }
}

/// 本机回环的 QUIC 配置（自签证书 + 回环地址）。
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

/// 测试用控制通道：真实 QUIC 双向流（§5 控制 stream 0）。
///
/// 分帧用 [`beam_net::framing`]（线上格式的唯一真值来源）；
/// 读写都是**非阻塞**的——会话层在 `pump()` 里同步调用，
/// 因此这里用 `poll_read` / `poll_write` + 空 waker 实现「有数据才动」。
struct QuicControl {
    send: Box<dyn AsyncWrite + Unpin + Send>,
    recv: Box<dyn AsyncRead + Unpin + Send>,
    /// 待发字节（发送缓冲满时留到下次 `flush` 续写）。
    out: Vec<u8>,
    /// 增量分帧器。
    decoder: FrameDecoder,
}

impl QuicControl {
    fn new(
        send: Box<dyn AsyncWrite + Unpin + Send>,
        recv: Box<dyn AsyncRead + Unpin + Send>,
    ) -> Self {
        Self {
            send,
            recv,
            out: Vec::new(),
            decoder: FrameDecoder::new(),
        }
    }

    /// 非阻塞写：尽量把待发字节写进流里。
    fn flush_write(&mut self) -> Result<(), SessionError> {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        while !self.out.is_empty() {
            let written = {
                let data = &self.out[..];
                match Pin::new(&mut *self.send).poll_write(&mut cx, data) {
                    Poll::Ready(Ok(n)) => n,
                    Poll::Ready(Err(e)) => {
                        return Err(SessionError::TransportFailed {
                            reason: format!("写控制流失败: {e}"),
                        });
                    }
                    Poll::Pending => break,
                }
            };
            if written == 0 {
                break;
            }
            self.out.drain(..written);
        }
        Ok(())
    }

    /// 非阻塞读：读一点字节，切出一条控制消息。
    fn poll_message(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        let mut raw = [0u8; 4096];
        let filled = {
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            let mut buf = ReadBuf::new(&mut raw);
            match Pin::new(&mut *self.recv).poll_read(&mut cx, &mut buf) {
                Poll::Ready(Ok(())) => buf.filled().len(),
                Poll::Ready(Err(e)) => {
                    return Some(Err(SessionError::TransportFailed {
                        reason: format!("读控制流失败: {e}"),
                    }));
                }
                Poll::Pending => 0,
            }
        };
        if filled > 0 {
            self.decoder.push(&raw[..filled]);
        }
        match self.decoder.next_message::<ControlMsg>() {
            Ok(Some(msg)) => Some(Ok(msg)),
            Ok(None) => None,
            Err(e) => Some(Err(SessionError::TransportFailed {
                reason: format!("解码控制消息失败: {e}"),
            })),
        }
    }
}

impl ControlChannel for QuicControl {
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        let frame = beam_net::encode_frame(&msg).map_err(|e| SessionError::TransportFailed {
            reason: format!("编码控制消息失败: {e}"),
        })?;
        self.out.extend_from_slice(&frame);
        self.flush_write()
    }

    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        self.poll_message()
    }

    fn flush(&mut self) -> Result<(), SessionError> {
        self.flush_write()
    }
}

/// 客户端侧传输：控制走双向流，画面走 datagram（§5 三通道）。
struct QuicTransport {
    control: QuicControl,
    /// 持久持有一个 `VideoStream`：`Conn::video_stream()` 每次调用都会新建重组器，
    /// 中途新建会丢掉「已收到但尚未重组完」的分片。
    video: beam_net::VideoStream,
}

impl ControlChannel for QuicTransport {
    fn send_control(&mut self, msg: ControlMsg) -> Result<(), SessionError> {
        self.control.send_control(msg)
    }

    fn recv_control(&mut self) -> Option<Result<ControlMsg, SessionError>> {
        self.control.recv_control()
    }

    fn flush(&mut self) -> Result<(), SessionError> {
        self.control.flush()
    }
}

impl Transport for QuicTransport {
    fn send_frame(&mut self, _f: EncodedFrame) -> Result<(), SessionError> {
        Ok(())
    }

    fn send_input(&mut self, _ev: InputEvent) -> Result<(), SessionError> {
        Ok(())
    }

    fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
        match self.video.try_next_frame() {
            Ok(Some(frame)) => Some(Ok(EncodedFrame {
                frame_id: frame.frame_id,
                pts_us: frame.pts_us,
                capture_ts_us: frame.capture_ts_us,
                is_key: frame.is_key,
                slices: vec![frame.payload],
            })),
            Ok(None) => None,
            Err(e) => Some(Err(SessionError::TransportFailed {
                reason: format!("取帧失败: {e}"),
            })),
        }
    }

    fn take_dropped_frames(&mut self) -> u32 {
        // §3.2 E2004：重组超时的帧由 `VideoStream` 记账，这里取出来交给会话层上报。
        self.video.take_expired().len() as u32
    }
}

/// 免去同步工厂里的阻塞连接。
struct FixedFactory {
    transport: Option<Box<dyn Transport>>,
}

impl TransportFactory for FixedFactory {
    fn connect(&mut self) -> Result<Box<dyn Transport>, SessionError> {
        self.transport
            .take()
            .ok_or_else(|| SessionError::MissingDependency {
                what: "transport".to_string(),
            })
    }
}

/// 主机侧发送器：画面走真实 QUIC datagram（§5）。
struct DatagramSender {
    conn: beam_net::Conn,
}

impl VideoSender for DatagramSender {
    fn send_frame(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        self.conn
            .send_frame(f)
            .map_err(|e| SessionError::TransportFailed {
                reason: format!("发画面失败: {e}"),
            })
    }

    fn send_control(&mut self, _msg: ControlMsg) -> Result<(), SessionError> {
        Ok(())
    }
}

/// 主机侧流水线：产出指定的一帧，并统计 IDR 请求次数。
struct OneFramePipeline {
    frame: Option<EncodedFrame>,
    idr_requests: Arc<AtomicUsize>,
}

impl FramePipeline for OneFramePipeline {
    fn start(&mut self) -> Result<(), SessionError> {
        Ok(())
    }

    fn poll(&mut self) -> Option<EncodedFrame> {
        self.frame.take()
    }

    fn request_idr(&mut self) {
        self.idr_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn stop(&mut self) {}
}

/// 客户端侧记录收到的画面帧。
#[derive(Clone, Default)]
struct RecordingConsumer {
    received: Arc<std::sync::Mutex<Vec<EncodedFrame>>>,
}

impl FrameConsumer for RecordingConsumer {
    fn consume(&mut self, f: EncodedFrame) -> Result<(), SessionError> {
        self.received.lock().expect("锁").push(f);
        Ok(())
    }
}

/// 造一帧画面。
fn frame(frame_id: u32, is_key: bool) -> EncodedFrame {
    EncodedFrame {
        frame_id,
        pts_us: 5000,
        capture_ts_us: 5000,
        is_key,
        slices: vec![Bytes::from_static(b"session-frame")],
    }
}

/// 回环里的一对会话：客户端会话 + 主机会话 + 主机流水线的 IDR 计数。
struct Loopback {
    client: beam_session::SessionHandle,
    host: beam_session::SessionHandle,
    idr_requests: Arc<AtomicUsize>,
    received: Arc<std::sync::Mutex<Vec<EncodedFrame>>>,
}

/// 起一对会话：真实 QUIC 连接 + 真实控制流 + 真实画面 datagram。
///
/// `first_frame` 是主机流水线产出的那一帧（决定它是不是 IDR）。
async fn start_loopback(first_frame: EncodedFrame) -> Loopback {
    let (host_conn, client_conn) = quic_pair().await;

    // 客户端：开控制双向流（§5 控制 stream 0）。
    let (client_send, client_recv) = client_conn.open_bi().await.expect("客户端应当能开控制流");
    let consumer = RecordingConsumer::default();
    let received = consumer.received.clone();
    let client_transport = QuicTransport {
        control: QuicControl::new(Box::new(client_send), Box::new(client_recv)),
        video: client_conn.video_stream(),
    };
    let client = ClientSession::connect_with_control(
        PeerAddr {
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
                transport: Some(Box::new(client_transport)),
            })),
            input: None,
        },
        ClientControlDeps::default(),
    );

    // 主机：接受控制双向流（QUIC 的流要等对端发数据才可见，
    // 客户端会话建立时会立刻发 `Hello`，因此这里能返回）。
    let (host_send, host_recv) = host_conn.accept_bi().await.expect("主机应当能接受控制流");
    let idr_requests = Arc::new(AtomicUsize::new(0));
    let host = HostSession::start_with_control(
        HostSessionConfig {
            stream: host_stream(),
            ..HostSessionConfig::default()
        },
        HostDeps {
            pipeline: Box::new(OneFramePipeline {
                frame: Some(first_frame),
                idr_requests: Arc::clone(&idr_requests),
            }),
            sender: Box::new(DatagramSender {
                conn: host_conn.clone(),
            }),
        },
        Some(HostControlDeps {
            control: Box::new(QuicControl::new(Box::new(host_send), Box::new(host_recv))),
            pairing: PairingConfig::default(),
            rate: None,
        }),
    );
    host.apply(SessionInput::Connecting)
        .expect("应当能进入连接态");
    host.apply(SessionInput::Connected)
        .expect("应当能进入协商态");

    Loopback {
        client,
        host,
        idr_requests,
        received,
    }
}

/// 两端一起推进，直到 `done` 为真或超时。
async fn pump_until(loopback: &Loopback, done: impl Fn(&Loopback) -> bool) -> bool {
    for _ in 0..400 {
        loopback.client.pump().expect("客户端推进");
        loopback.host.pump().expect("主机推进");
        if done(loopback) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

/// §4.1：真实 QUIC 上 `Hello` → `HelloAck` → 两端进入 `Streaming`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_session_negotiates_over_real_quic() {
    let loopback = start_loopback(frame(1, true)).await;
    let mut client_events = loopback.client.events();

    let negotiated = pump_until(&loopback, |l| {
        l.client.state() == SessionState::Streaming && l.host.state() == SessionState::Streaming
    })
    .await;
    assert!(
        negotiated,
        "两端都应当进入串流态（客户端={}，主机={}）",
        loopback.client.state(),
        loopback.host.state()
    );

    // 客户端应当收到 `Negotiated` 事件（能力协商真的跑了一次）。
    let mut saw_negotiated = false;
    while let Some(ev) = client_events.next().await {
        if let SessionEvent::Negotiated { config } = ev {
            assert_eq!(config.width, 1280, "协商结果取双方分辨率的较小值");
            assert_eq!(config.height, 720);
            assert_eq!(config.fps, 30);
            saw_negotiated = true;
            break;
        }
    }
    assert!(saw_negotiated, "客户端必须收到 Negotiated 事件");
}

/// §4.1「视频 datagram(首帧 IDR)」：首帧不是 IDR → 客户端请求一次 IDR。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_first_frame_not_idr_triggers_one_idr_request() {
    // 首帧**不是**关键帧。
    let loopback = start_loopback(frame(7, false)).await;

    let got_frame = pump_until(&loopback, |l| !l.received.lock().expect("锁").is_empty()).await;
    assert!(got_frame, "客户端会话必须收到主机发出的画面帧");
    let frames = loopback.received.lock().expect("锁").clone();
    assert!(!frames[0].is_key, "本用例的首帧必须是非关键帧");

    // 客户端应当因首帧非 IDR 发一次 `RequestIdr`，主机因此补一个 IDR。
    let asked = pump_until(&loopback, |l| l.idr_requests.load(Ordering::SeqCst) > 0).await;
    assert!(asked, "客户端必须因首帧非 IDR 请求一次 IDR");
    assert_eq!(
        loopback.idr_requests.load(Ordering::SeqCst),
        1,
        "只请求一次 IDR,不得刷请求"
    );
}
