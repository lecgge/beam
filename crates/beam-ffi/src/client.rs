//! FFI 导出面（§3.12）。
//!
//! 这一层只做「类型 ↔ Kotlin」的映射与回调桥接，不含业务逻辑。
//!
//! ## `connect` 真的起会话（§3.12）
//!
//! ```text
//! PeerInfo ─▶ 指纹固定（§3.3）
//!          ─▶ beam_net::NetEndpoint::connect   真实 QUIC + TLS1.3
//!          ─▶ beam_session::NetTransport        控制双向流 + 输入单向流 + 画面 datagram
//!          ─▶ beam_session::ClientSession       状态机 + 能力协商（cfg 真的参与）
//!          ─▶ 专用线程                          推进会话 + 回调 FrameSink（帧 / 事件）
//! ```
//!
//! ## 线程模型（§3.12「Rust tokio 运行时在 so 内」「帧回调在专用线程」）
//!
//! - [`BeamClient`] 持有一个**常驻 tokio 运行时**（多线程），QUIC 的收发由它的工作线程驱动；
//! - 帧与事件都在**同一个专用线程**（`beam-ffi-session`）上回调 `FrameSink`，
//!   调用方必须尽快返回，不要在回调里做重活。
//!
//! ## 身份与信任库（**边界**：仍需上层提供）
//!
//! - 本机 TLS 身份：当前用 [`beam_pair::Identity::generate`] **现场生成**，
//!   **不落盘**（`beam-ffi` 里没有密钥存储后端）；
//! - 信任库：当前恒为空，对端可信与否由**本次连接固定的指纹**决定
//!   （指纹来自扫码配对载荷）。
//!
//! 这两件事（身份持久化 / 信任库写入）必须由上层（Android `core-data`）提供，
//! 本层没有假装已经持久化。

// crate 级 lint 在 `uniffi` feature 打开时放宽为 `allow`（uniffi scaffolding 含 unsafe impl），
// 这里用模块级 `deny` 把业务代码重新收紧。
#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beam_discovery::{DiscoveredHost, DiscoveryEvent};
use beam_net::{NetConfig, NetEndpoint, TlsIdentity};
use beam_pair::{Fingerprint, Identity, PairPayload, PeerVerifier, TrustStore};
use beam_proto::StreamConfig;
use beam_session::{PeerAddr, SessionEvent, SessionHandle};

use crate::error::FfiError;

/// §5 的 TLS server name。
///
/// 所有 Beam 身份的证书 SAN 都是 `beam.local`（`beam_pair::Identity` 生成时写死），
/// 因此不能把 mDNS 里的展示名当 server name 用。
pub const TLS_SERVER_NAME: &str = "beam.local";

/// 会话推进线程的节拍（毫秒）。
///
/// 会话状态机是**同步**的（`apply` / `pump`），必须有人不停地推它；
/// 这个节拍只影响「取事件 / 收帧」的延迟，不影响吞吐
/// （每次 `pump` 都会把已到达的帧一次性取完）。
const PUMP_INTERVAL: Duration = Duration::from_millis(4);

/// 客户端配置（§3.12 `BeamClient::new(config)`）。
#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    /// 设备名（展示用）。
    pub device_name: String,
    /// 平台标识（`android` / `windows`）。
    pub platform: String,
    /// 是否自动重连。
    pub auto_reconnect: bool,
    /// 是否启用日志。
    pub enable_logging: bool,
}

/// 配对后的主机信息（跨 FFI 传递）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// 主机地址。
    pub host: String,
    /// 主机端口。
    pub port: u16,
    /// 主机证书指纹（十六进制）。
    pub fingerprint: String,
}

impl PeerInfo {
    /// 转成会话层使用的地址。
    pub fn to_peer_addr(&self) -> PeerAddr {
        PeerAddr {
            host: self.host.clone(),
            port: self.port,
            fingerprint: Some(self.fingerprint.clone()),
        }
    }

    /// 从会话层地址构造。
    pub fn from_peer_addr(p: &PeerAddr) -> Self {
        Self {
            host: p.host.clone(),
            port: p.port,
            fingerprint: p.fingerprint.clone().unwrap_or_default(),
        }
    }
}

/// 发现事件回调（§3.12 `browse(listener)`）。
pub trait DiscoveryListener: Send {
    /// 发现一个主机。
    fn on_host_found(&mut self, host: &DiscoveredHost);
    /// 主机消失。
    fn on_host_lost(&mut self, fullname: &str);
}

/// 帧 / 事件回调（§3.12 `callback interface FrameSink`）。
///
/// 帧回调在专用线程上触发（§3.12「帧回调在专用线程 → `queue()`」），
/// 调用方必须尽快返回，不要在回调里做重活。
pub trait FrameSink: Send {
    /// 收到一帧画面。
    ///
    /// `data` 是**编码后的码流**（H.264 / HEVC / AV1）：解码在 Kotlin 侧做
    /// （§3.12「不负责：解码 / 渲染」），`key` 是 IDR 标记（§4.1 首帧必须是 IDR）。
    ///
    /// `data` 在热路径上应当是 JNI DirectByteBuffer 指向的 Rust 侧内存，
    /// 详见 [`crate`] 的模块文档；调用方消费完后需要归还缓冲。
    fn on_frame(&mut self, data: &[u8], pts_us: u64, capture_ts_us: u64, key: bool);

    /// 收到会话事件。
    fn on_event(&mut self, event: SessionEvent);
}

/// 事件桥接：把会话事件转成 `FrameSink` 回调。
#[derive(Debug)]
pub struct EventBridge<S> {
    sink: S,
}

impl<S> EventBridge<S>
where
    S: FrameSink,
{
    /// 组装桥接。
    pub fn new(sink: S) -> Self {
        Self { sink }
    }

    /// 转发一帧。
    pub fn forward_frame(&mut self, data: &[u8], pts_us: u64, capture_ts_us: u64, key: bool) {
        self.sink.on_frame(data, pts_us, capture_ts_us, key);
    }

    /// 转发一个事件。
    pub fn forward_event(&mut self, event: SessionEvent) {
        self.sink.on_event(event);
    }
}

/// 客户端的对端证书校验器（§3.3「客户端只信已配对主机的指纹」）。
///
/// 可信来源有两处：
///
/// 1. **信任库**（[`TrustStore`]）：已配对主机，**由上层写入**（本层当前恒为空）；
/// 2. **本次连接固定的指纹**：来自扫码配对载荷（[`PeerInfo::fingerprint`]）。
#[derive(Debug)]
struct ClientPeerVerifier {
    trust: TrustStore,
    pinned: Option<Fingerprint>,
}

impl PeerVerifier for ClientPeerVerifier {
    fn verify(&self, fp: &Fingerprint) -> bool {
        self.trust.is_trusted(fp) || self.pinned.as_ref() == Some(fp)
    }
}

/// 把会话帧转成 `FrameSink` 回调。
///
/// 帧数据是**编码后的码流**：解码在 Kotlin 侧做（§3.12），
/// `key` 是 IDR 标记（§4.1「首帧 IDR」）。
struct SinkConsumer {
    sink: Arc<Mutex<Box<dyn FrameSink>>>,
}

impl beam_session::FrameConsumer for SinkConsumer {
    fn consume(
        &mut self,
        frame: beam_proto::EncodedFrame,
    ) -> Result<(), beam_session::SessionError> {
        let payload = frame.to_payload();
        let mut guard = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        guard.on_frame(&payload, frame.pts_us, frame.capture_ts_us, frame.is_key);
        Ok(())
    }
}

/// 把**已经建好的** QUIC 连接包装成会话传输的工厂。
///
/// ## 为什么工厂里没有 `await`
///
/// 会话层的 [`beam_session::TransportFactory::connect`] 是**同步**接口，
/// 而 `beam_net::NetEndpoint::connect`（建链）与
/// `beam_session::NetTransport::connect`（开控制双向流 + 输入单向流）都是**异步**的。
///
/// 因此**连接在工厂之外先建好**（见 [`BeamClient::connect`]），
/// 工厂只负责把已经建好的 [`beam_net::Conn`] 包成 [`beam_session::NetTransport`]。
///
/// 这里的 `handle.block_on` 在**调用方线程**上驱动那个异步 future：
/// 调用方线程此时不在 tokio runtime 内部（会话的 `connect_with` 是同步调用），
/// QUIC 的收发由 `BeamClient` 持有的常驻 runtime 的工作线程驱动。
struct ConnTransportFactory {
    conn: Option<beam_net::Conn>,
    handle: tokio::runtime::Handle,
}

impl beam_session::TransportFactory for ConnTransportFactory {
    fn connect(&mut self) -> Result<Box<dyn beam_session::Transport>, beam_session::SessionError> {
        let conn =
            self.conn
                .take()
                .ok_or_else(|| beam_session::SessionError::MissingDependency {
                    what: "transport".to_string(),
                })?;
        let transport = self
            .handle
            .block_on(beam_session::NetTransport::connect(conn))?;
        Ok(Box::new(transport))
    }
}

/// Beam 客户端（§3.12 导出面）。
pub struct BeamClient {
    config: ClientConfig,
    trust: TrustStore,
    /// 本机 TLS 身份（§3.3）。
    ///
    /// **当前不持久化**：每次 [`BeamClient::new`] 现场生成一份新的自签身份。
    /// 持久化必须由上层提供存储后端（`beam_pair::KeyStore`）后接上。
    identity: Identity,
    /// 常驻 tokio 运行时（§3.12「Rust tokio 运行时在 so 内」）。
    runtime: tokio::runtime::Runtime,
}

impl std::fmt::Debug for BeamClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BeamClient")
            .field("config", &self.config)
            .field("identity", &self.identity)
            .finish()
    }
}

impl BeamClient {
    /// 创建客户端（§3.12 `BeamClient::new(config)`）。
    pub fn new(config: ClientConfig) -> Result<Self, FfiError> {
        let identity = Identity::generate().map_err(|e| FfiError::ConnectFailed {
            reason: format!("生成本机 TLS 身份失败: {e}"),
        })?;
        // §3.12「Rust tokio 运行时在 so 内」：常驻运行时随客户端一起活着，
        // QUIC 的收发由它的工作线程驱动。
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| FfiError::ConnectFailed {
                reason: format!("启动 tokio 运行时失败: {e}"),
            })?;
        Ok(Self {
            config,
            trust: TrustStore::new(),
            identity,
            runtime,
        })
    }

    /// 客户端配置。
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// 信任库（配对成功后由上层写入，这里只读）。
    ///
    /// **当前恒为空**：本层还没有「配对成功后写指纹」的通路，
    /// 连接时的可信判定靠 [`PeerInfo::fingerprint`] 固定的指纹。
    pub fn trust_store(&self) -> &TrustStore {
        &self.trust
    }

    /// 本机 TLS 身份（§3.3）。
    ///
    /// **当前不持久化**：见 [`BeamClient`] 的字段说明。
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// 开始浏览局域网主机（§3.12 `browse(listener)`）。
    ///
    /// 浏览循环在后台线程里跑，事件通过 `listener` 回调给 Kotlin 侧。
    pub fn browse(&self, listener: Box<dyn DiscoveryListener>) -> Result<(), FfiError> {
        let mut browser =
            beam_discovery::Browser::start().map_err(|_| FfiError::DiscoveryFailed)?;
        std::thread::Builder::new()
            .name("beam-ffi-browse".to_string())
            .spawn(move || {
                use futures::StreamExt;
                let mut listener = listener;
                while let Some(event) = futures::executor::block_on(browser.next()) {
                    match event {
                        DiscoveryEvent::Found(host) => listener.on_host_found(&host),
                        DiscoveryEvent::Lost { fullname } => listener.on_host_lost(&fullname),
                    }
                }
            })
            .map_err(|_| FfiError::DiscoveryFailed)?;
        Ok(())
    }

    /// 扫码配对（§3.12 `pair_qr(payload)`）。
    ///
    /// 载荷格式见 §3.3：`beam://pair?h=ip:port&fp=..&t=token`。
    pub fn pair_qr(&self, payload: &str) -> Result<PeerInfo, FfiError> {
        let parsed = PairPayload::from_uri(payload).map_err(|e| FfiError::InvalidPayload {
            reason: e.to_string(),
        })?;
        Ok(PeerInfo {
            host: parsed.host,
            port: parsed.port,
            fingerprint: parsed.fingerprint.to_hex(),
        })
    }

    /// PIN 配对（§3.12 `pair_pin(peer, pin)`）。
    ///
    /// 这里只做本地校验（PIN 格式 + 指纹格式）；真正的 SPAKE2 交换由会话层
    /// 经 net 传递消息完成（§3.3「PIN 交换消息由 session 经 net 传递」）。
    pub fn pair_pin(&self, peer: &PeerInfo, pin: &str) -> Result<(), FfiError> {
        let policy = beam_pair::PairingPolicy::default();
        beam_pair::validate_pin(pin, &policy).map_err(|_| FfiError::PinFailed)?;
        if !peer.fingerprint.is_empty() {
            Fingerprint::from_hex(&peer.fingerprint).map_err(|e| FfiError::InvalidPayload {
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }

    /// 连接主机（§3.12 `connect(peer, cfg, sink)`）。
    ///
    /// 这一步**真的建立会话**：
    ///
    /// 1. **固定主机身份**（§3.3 指纹固定）：本次连接只接受信任库里的指纹，
    ///    或配对载荷里给出的那一个指纹；
    /// 2. **建链**：用本机 TLS 身份发起真实 QUIC 连接（`beam_net::NetEndpoint::connect`）；
    /// 3. **传输**：把连接包成会话传输（`beam_session::NetTransport`：
    ///    控制双向流 + 输入单向流 + 画面 datagram）；
    /// 4. **会话**：`beam_session::ClientSession::connect_with`，`cfg` 作为串流配置
    ///    **真的**参与会话（能力协商由它推导）；
    /// 5. **回调**：专用线程推进会话，把帧与事件回调给 `sink`。
    pub fn connect(
        &self,
        peer: PeerInfo,
        cfg: StreamConfig,
        sink: Box<dyn FrameSink>,
    ) -> Result<SessionHandle, FfiError> {
        let pinned = pinned_fingerprint(&peer.fingerprint)?;
        let verifier: Arc<dyn PeerVerifier> = Arc::new(ClientPeerVerifier {
            trust: self.trust.clone(),
            pinned,
        });
        // 建链是异步的（QUIC 握手必须 await），而会话的传输工厂是同步的：
        // 因此**连接在工厂之外先建好**，工厂只负责把已经建好的连接包成传输。
        let conn = self.runtime.block_on(async {
            let addr = resolve_addr(&peer.host, peer.port).await?;
            NetEndpoint::connect(addr, self.net_config(), verifier)
                .await
                .map_err(|e| FfiError::ConnectFailed {
                    reason: format!("QUIC 握手失败 {}: {}", e.code_str(), e.text()),
                })
        })?;
        let factory = ConnTransportFactory {
            conn: Some(conn),
            handle: self.runtime.handle().clone(),
        };
        self.connect_with_transport(peer, cfg, sink, Box::new(factory))
    }

    /// 组装会话并注入传输（内部入口）。
    ///
    /// 与 [`BeamClient::connect`] 的唯一区别是**传输由调用方注入**：
    /// 单测用假传输就能证明「cfg 真的生效」「sink 真的被回调」，
    /// 不需要真机 / 真主机。
    fn connect_with_transport(
        &self,
        peer: PeerInfo,
        cfg: StreamConfig,
        sink: Box<dyn FrameSink>,
        factory: Box<dyn beam_session::TransportFactory>,
    ) -> Result<SessionHandle, FfiError> {
        // sink 有两个使用方：会话帧（消费者）与会话事件（后台线程）。
        // 用一把锁共享同一个 sink，保证帧与事件不会并发进入回调。
        let sink = Arc::new(Mutex::new(sink));
        let consumer = Box::new(SinkConsumer {
            sink: Arc::clone(&sink),
        });
        let session_cfg = beam_session::ClientSessionConfig {
            // §3.9：串流配置**真的**进入会话配置（能力协商由它推导）。
            stream: cfg,
            ..beam_session::ClientSessionConfig::default()
        };
        let deps = beam_session::ClientDeps {
            consumer: Some(consumer),
            transport: Some(factory),
        };
        let handle =
            beam_session::ClientSession::connect_with(peer.to_peer_addr(), session_cfg, deps);
        if handle.state() == beam_session::SessionState::Idle {
            // 传输工厂失败：会话没有推进（§3.9「没有传输就无从连接」）。
            return Err(FfiError::ConnectFailed {
                reason: "传输建立失败".to_string(),
            });
        }
        spawn_session_worker(handle.clone(), Arc::clone(&sink))?;
        Ok(handle)
    }

    /// 组装网络参数（§3.2）。
    ///
    /// TLS 身份来自本机 [`Identity`]（§3.3）；server name 固定为 `beam.local`
    /// （所有 Beam 身份的证书 SAN 都是它）。
    fn net_config(&self) -> NetConfig {
        NetConfig {
            server_name: TLS_SERVER_NAME.to_string(),
            // §3.2：两端都要有 TLS 身份，否则 E2007。
            identity: Some(TlsIdentity {
                cert_chain: self.identity.certificate_chain(),
                key: self.identity.private_key(),
            }),
            ..NetConfig::default()
        }
    }
}

/// 取出本次连接要固定的主机指纹（§3.3「证书指纹固定」）。
///
/// 指纹来自扫码配对载荷（[`PeerInfo::fingerprint`]）。
/// 为空表示「上层还没给出指纹」——此时只信任信任库里的主机。
/// 指纹格式非法则直接失败：宁可连不上，也不要在身份不明时握手。
fn pinned_fingerprint(raw: &str) -> Result<Option<Fingerprint>, FfiError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    Fingerprint::from_hex(raw)
        .map(Some)
        .map_err(|e| FfiError::ConnectFailed {
            reason: format!("主机指纹非法: {e}"),
        })
}

/// 解析主机地址（域名与 IP 都要支持）。
async fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr, FfiError> {
    let mut addrs =
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| FfiError::ConnectFailed {
                reason: format!("解析主机地址 {host}:{port} 失败: {e}"),
            })?;
    addrs.next().ok_or_else(|| FfiError::ConnectFailed {
        reason: format!("主机地址 {host}:{port} 没有可用地址"),
    })
}

/// 会话后台线程：推进状态机，并把帧与事件回调给 `sink`。
///
/// §3.12「帧回调在专用线程」：帧与事件都在**这一个**专用线程上回调，
/// 调用方（Kotlin）不得在回调里做重活。
///
/// 线程的退出条件：
///
/// - 会话被结束（`stop()` / 对端 `Bye` → `Closed`）；
/// - 会话出错（`Error`）；
/// - **调用方已经放开了会话句柄**（此时会话与回调对象都该被释放）。
fn spawn_session_worker(
    handle: SessionHandle,
    sink: Arc<Mutex<Box<dyn FrameSink>>>,
) -> Result<(), FfiError> {
    use futures::{FutureExt, StreamExt};
    std::thread::Builder::new()
        .name("beam-ffi-session".to_string())
        .spawn(move || {
            let mut events = handle.events();
            while !handle.is_stopped()
                && handle.state() != beam_session::SessionState::Closed
                && handle.state() != beam_session::SessionState::Error
                && !handle.is_last_handle()
            {
                // 推进一步：收控制消息 / 取帧。
                let _ = handle.pump();
                // 非阻塞取事件：有就转发，没有就等下一轮。
                while let Some(Some(event)) = events.next().now_or_never() {
                    if let Ok(mut guard) = sink.lock() {
                        guard.on_event(event);
                    }
                }
                std::thread::sleep(PUMP_INTERVAL);
            }
        })
        .map(|_| ())
        .map_err(|e| FfiError::ConnectFailed {
            reason: format!("启动会话线程失败: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc;

    use beam_proto::{ControlMsg, EncodedFrame};
    use beam_session::{ControlChannel, SessionError, Transport, TransportFactory};

    struct NoopSink;
    impl FrameSink for NoopSink {
        fn on_frame(&mut self, _data: &[u8], _pts_us: u64, _capture_ts_us: u64, _key: bool) {}
        fn on_event(&mut self, _event: SessionEvent) {}
    }

    /// 记录帧与事件的 sink（测试用）。
    struct RecordingSink {
        frames: mpsc::Sender<(Vec<u8>, u64, u64, bool)>,
        events: mpsc::Sender<SessionEvent>,
    }

    impl FrameSink for RecordingSink {
        fn on_frame(&mut self, data: &[u8], pts_us: u64, capture_ts_us: u64, key: bool) {
            let _ = self
                .frames
                .send((data.to_vec(), pts_us, capture_ts_us, key));
        }
        fn on_event(&mut self, event: SessionEvent) {
            let _ = self.events.send(event);
        }
    }

    /// 可脚本化的假传输：控制消息进队列，发出的控制消息进共享日志。
    struct FakeTransport {
        inbound: VecDeque<Result<ControlMsg, SessionError>>,
        sent: Arc<Mutex<Vec<ControlMsg>>>,
        frames: VecDeque<EncodedFrame>,
    }

    impl FakeTransport {
        fn new() -> Self {
            Self {
                inbound: VecDeque::new(),
                sent: Arc::new(Mutex::new(Vec::new())),
                frames: VecDeque::new(),
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

        fn send_input(&mut self, _ev: beam_proto::InputEvent) -> Result<(), SessionError> {
            Ok(())
        }

        fn recv_frame(&mut self) -> Option<Result<EncodedFrame, SessionError>> {
            self.frames.pop_front().map(Ok)
        }
    }

    /// 假传输工厂：把一个写好的假传输交给会话层。
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

    fn peer() -> PeerInfo {
        PeerInfo {
            host: "192.168.1.10".into(),
            port: 4827,
            fingerprint: String::new(),
        }
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

    #[test]
    fn new_client_works() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        assert_eq!(client.config().platform, "");
    }

    #[test]
    fn pair_qr_parses_payload() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let fp = Fingerprint::from_bytes([0xAB; 32]);
        let payload = PairPayload {
            host: "192.168.1.10".into(),
            port: 4827,
            fingerprint: fp,
            token: "tok".into(),
        };
        let peer = client.pair_qr(&payload.to_uri()).expect("应当能解析");
        assert_eq!(peer.host, "192.168.1.10");
        assert_eq!(peer.port, 4827);
        assert_eq!(peer.fingerprint, fp.to_hex());
    }

    #[test]
    fn pair_qr_rejects_garbage() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let err = client.pair_qr("not a uri").expect_err("必须失败");
        assert_eq!(err.code(), 14001);
    }

    #[test]
    fn pair_pin_validates_length() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let peer = PeerInfo {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: String::new(),
        };
        let err = client.pair_pin(&peer, "1").expect_err("PIN 太短必须失败");
        assert_eq!(err.code(), 14002);
    }

    /// §3.9：串流配置必须**真的**进入会话，而不是被丢掉。
    #[test]
    fn connect_uses_the_given_stream_config() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let cfg = StreamConfig {
            width: 1280,
            height: 720,
            fps: 90,
            bitrate_bps: 12_000_000,
            codec: beam_proto::CodecKind::Hevc,
            audio_enabled: true,
            features: 0,
        };
        let transport = FakeTransport::new();
        let sent = Arc::clone(&transport.sent);
        let handle = client
            .connect_with_transport(
                peer(),
                cfg.clone(),
                Box::new(NoopSink),
                Box::new(FakeFactory {
                    transport: Some(Box::new(transport)),
                }),
            )
            .expect("应当能连上");

        assert_eq!(handle.config(), cfg, "串流配置必须原样进入会话");
        assert_eq!(
            handle.state(),
            beam_session::SessionState::Negotiating,
            "连上传输后必须推进到能力协商"
        );

        // 能力集也必须由这份配置推导（§3.1 `Caps`）。
        let log = sent.lock().expect("锁").clone();
        let caps = log
            .iter()
            .find_map(|m| match m {
                ControlMsg::Hello(hello) => Some(hello.caps.clone()),
                _ => None,
            })
            .expect("连接后必须发出 Hello");
        assert_eq!(caps.max_fps, 90);
        assert_eq!(caps.max_res.width, 1280);
        assert_eq!(caps.max_res.height, 720);
        assert_eq!(caps.codecs, vec![beam_proto::CodecKind::Hevc]);

        handle.stop();
    }

    /// `FrameSink::on_frame` 必须真的被调用，且参数原样送达。
    #[test]
    fn sink_receives_frames() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let (frame_tx, frame_rx) = mpsc::channel();
        let (event_tx, _event_rx) = mpsc::channel();
        let sink = RecordingSink {
            frames: frame_tx,
            events: event_tx,
        };

        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));
        transport.frames.push_back(EncodedFrame {
            frame_id: 7,
            pts_us: 1_000,
            capture_ts_us: 900,
            is_key: true,
            slices: vec![bytes::Bytes::from_static(b"fake-frame")],
        });

        let handle = client
            .connect_with_transport(
                peer(),
                StreamConfig::default(),
                Box::new(sink),
                Box::new(FakeFactory {
                    transport: Some(Box::new(transport)),
                }),
            )
            .expect("应当能连上");

        let (data, pts_us, capture_ts_us, key) = frame_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("必须回调帧");
        assert_eq!(data, b"fake-frame", "帧数据必须原样送达");
        assert_eq!(pts_us, 1_000);
        assert_eq!(capture_ts_us, 900);
        assert!(key, "关键帧标记必须原样送达");

        handle.stop();
    }

    /// `FrameSink::on_event` 必须真的被调用（会话事件经它转给 Kotlin）。
    #[test]
    fn sink_receives_events() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let (frame_tx, _frame_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let sink = RecordingSink {
            frames: frame_tx,
            events: event_tx,
        };

        let mut transport = FakeTransport::new();
        transport.inbound.push_back(Ok(hello_ack()));

        let handle = client
            .connect_with_transport(
                peer(),
                StreamConfig::default(),
                Box::new(sink),
                Box::new(FakeFactory {
                    transport: Some(Box::new(transport)),
                }),
            )
            .expect("应当能连上");

        // 事件是异步到达的：等到「能力协商完成」这一条为止。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut negotiated = None;
        while std::time::Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(SessionEvent::Negotiated { config }) => {
                    negotiated = Some(config);
                    break;
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        let config = negotiated.expect("必须回调协商事件");
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);

        handle.stop();
    }

    /// 指纹非法时必须在**握手之前**失败（身份不明不握手，§3.3）。
    #[test]
    fn connect_rejects_invalid_fingerprint() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let peer = PeerInfo {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: "zz".into(),
        };
        let err = client
            .connect(peer, StreamConfig::default(), Box::new(NoopSink))
            .expect_err("非法指纹必须失败");
        assert_eq!(err.code(), 14003);
    }

    #[test]
    fn event_bridge_forwards() {
        let mut bridge = EventBridge::new(NoopSink);
        bridge.forward_frame(&[1, 2, 3], 1, 2, true);
        bridge.forward_event(SessionEvent::Closed);
    }
}
