//! 音频 datagram 的本机回环集成测试（§5 三通道 + §3.7 音频）。
//!
//! 在本机起一对**真实**的 QUIC 端点（[`beam_net::NetEndpoint::listen`] +
//! [`beam_net::NetEndpoint::connect`]），验证音频链路真的接上了：
//!
//! - 主机侧用**真实的** [`beam_host::AudioPipeline`]（采集 → Opus → 数据报，
//!   采集 / 编码用假实现）把一帧「Opus」发出去；
//! - 客户端侧用**真实的** [`beam_client::DemuxTransport`] 收：
//!   音频数据报走音频支路，画面数据报走画面重组器；
//! - 断言：画面帧**完好**（音频数据报没有污染画面重组器），
//!   且音频数据报**逐字节**到达音频支路。
//!
//! 不依赖外网、不依赖真机音频设备，可重复运行。
//!
//! ## 没验证到的
//!
//! 这里验证的是**字节层面的接线**：数据报怎么装、怎么分流。
//! 「声音真的能出来」需要真机（真实 WASAPI loopback + 真实 Opus 编解码 +
//! 真实输出设备），本仓库的测试环境**没有**声卡回环验证条件。

use std::sync::Arc;
use std::time::Duration;

use beam_audio::{AudioConfig, AudioError, AudioSource, OpusEncoder, PcmFrame};
use beam_client::transport::DemuxTransport;
use beam_host::{AudioPipeline, NetAudioSender};
use beam_net::{NetConfig, NetEndpoint, TlsIdentity, VideoSender as NetVideoSender};
use beam_pair::{Identity, TrustStore, TrustedPeer};
use beam_proto::EncodedFrame;
use beam_session::{NetTransport, Transport};
use bytes::Bytes;

/// 假采集源：每次要一帧就给一帧固定内容的 PCM。
struct FakeSource {
    remaining: usize,
}

impl AudioSource for FakeSource {
    fn start(&mut self, _cfg: AudioConfig) -> Result<(), AudioError> {
        Ok(())
    }

    fn next_frame(&mut self) -> Option<PcmFrame> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some(PcmFrame {
            samples: Bytes::from_static(b"fake-opus-frame"),
            sample_rate: 48_000,
            channels: 2,
        })
    }

    fn stop(&mut self) {}
}

/// 假编码器：把 PCM 原样当成 Opus 负载。
struct FakeEncoder;

impl OpusEncoder for FakeEncoder {
    fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError> {
        Ok(pcm.samples.clone())
    }

    fn reset(&mut self) {}
}

/// 客户端侧的假音频接收端：记录收到的音频数据报。
#[derive(Clone, Default)]
struct RecordingAudio {
    received: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
}

impl beam_client::AudioDatagramSink for RecordingAudio {
    fn handle_datagram(&mut self, datagram: &[u8]) -> Result<(), AudioError> {
        self.received.lock().unwrap().push(datagram.to_vec());
        Ok(())
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audio_datagram_reaches_the_audio_sink_without_breaking_video() {
    let (host_conn, client_conn) = quic_pair().await;

    // 客户端：会话层那份传输（控制面 / 输入面）+ 带分流的数据报读点。
    let inner = NetTransport::connect(client_conn.clone())
        .await
        .expect("客户端应当能打开控制流");
    let recording = RecordingAudio::default();
    let received = recording.received.clone();
    let stats = beam_client::AudioReceiveStatsHandle::default();
    let mut transport = DemuxTransport::new(
        inner,
        client_conn.clone(),
        Some(Box::new(recording)),
        Some(stats.clone()),
    );

    // 主机：真实的音频流水线（采集 / 编码用假实现）→ 音频数据报。
    let mut pipeline = AudioPipeline::new(
        FakeSource { remaining: 1 },
        FakeEncoder,
        NetAudioSender::new(host_conn.clone()),
        AudioConfig {
            mode: beam_audio::AudioMode::HostAndStream,
            ..AudioConfig::default()
        },
    );
    pipeline.start().expect("主机音频应当能启动");
    assert!(
        pipeline.poll().expect("主机音频应当能发出"),
        "主机必须真的发出一条音频数据报"
    );

    // 主机：同时发一帧画面（同一条连接、同一个 datagram 通道）。
    let frame = EncodedFrame {
        frame_id: 7,
        pts_us: 1234,
        capture_ts_us: 1200,
        is_key: true,
        slices: vec![Bytes::from_static(b"fake-nal-payload")],
    };
    host_conn
        .send_frame(frame.clone())
        .expect("主机应当能发出画面");

    // 客户端：轮询取帧（音频数据报在同一次读取里被分流到音频支路）。
    let mut got: Option<EncodedFrame> = None;
    for _ in 0..400 {
        match transport.recv_frame() {
            Some(Ok(f)) => {
                got = Some(f);
                break;
            }
            Some(Err(e)) => panic!("取帧失败: {e}"),
            None => tokio::time::sleep(Duration::from_millis(5)).await,
        }
    }

    // 1. 画面帧必须完好：音频数据报没有污染画面重组器。
    let got = got.expect("客户端必须收到主机发出的画面帧");
    assert_eq!(got.frame_id, 7);
    assert_eq!(got.capture_ts_us, 1200);
    assert_eq!(got.to_payload().as_ref(), b"fake-nal-payload");

    // 2. 音频数据报必须**逐字节**到达音频支路。
    let audio = received.lock().unwrap();
    assert_eq!(audio.len(), 1, "客户端必须收到且只收到一条音频数据报");
    let dg = &audio[0];
    // 帧头 type = 1（音频），不进 FEC 分组。
    assert_eq!(dg[0], 0x11, "ver=1 / type=1（音频）");
    assert_eq!(dg[8], 1, "data_shards = 1");
    assert_eq!(dg[9], 0, "fec_shards = 0：音频不进 FEC 分组");
    // 负载就是 Opus 字节，没有 8 字节前缀。
    assert_eq!(&dg[16..], b"fake-opus-frame");

    // 3. 客户端侧的解析器能把它解回音频帧。
    let parsed = beam_audio::wire::decode_audio_datagram(dg).expect("应当能解析音频数据报");
    assert_eq!(parsed.payload.as_ref(), b"fake-opus-frame");
    assert_eq!(parsed.sequence(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audio_only_datagram_never_becomes_a_video_frame() {
    // 只有音频、没有画面时：客户端**不能**把音频数据报当成画面帧交出去。
    let (host_conn, client_conn) = quic_pair().await;

    let inner = NetTransport::connect(client_conn.clone())
        .await
        .expect("客户端应当能打开控制流");
    let recording = RecordingAudio::default();
    let received = recording.received.clone();
    let mut transport =
        DemuxTransport::new(inner, client_conn.clone(), Some(Box::new(recording)), None);

    let mut pipeline = AudioPipeline::new(
        FakeSource { remaining: 1 },
        FakeEncoder,
        NetAudioSender::new(host_conn.clone()),
        AudioConfig::default(),
    );
    pipeline.start().expect("主机音频应当能启动");
    pipeline.poll().expect("主机音频应当能发出");

    // 轮询若干轮：始终**不该**有画面帧。
    for _ in 0..50 {
        match transport.recv_frame() {
            Some(Ok(f)) => panic!("音频数据报不得被当成画面帧交出: {f:?}"),
            Some(Err(e)) => panic!("取帧失败: {e}"),
            None => {}
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let audio = received.lock().unwrap();
    assert_eq!(audio.len(), 1, "音频数据报必须到达音频支路");
}
