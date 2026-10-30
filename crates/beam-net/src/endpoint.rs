//! QUIC 端点与连接（§3.2 `NetEndpoint::listen/connect`、`VideoSender` / `VideoReceiver`）。
//!
//! §5：ALPN `beam/1`；控制 stream 0 双向可靠；视频 / 音频走 QUIC datagram。
//! §4：TLS 校验回调由 pair 提供（`PeerVerifier`），网络层不理解信任库语义。
//!
//! ## 线程模型：与 §3.2 的描述不同（登记项）
//!
//! §3.2 写的是「发送路径专用任务,UDP GSO 批量发包；接收重组单任务；
//! FEC 解码在 rayon 或 blocking 线程」。本 crate 的**实际实现方式**是：
//!
//! - **发送路径**：不额外开任务。`Conn::send_datagram` 是非阻塞的，
//!   数据报进入 quinn 连接自己的发送队列，由 quinn 的 per-connection driver
//!   任务统一发包 —— 「发送路径专用任务」事实上由 quinn 提供，
//!   再叠一层只会多一次队列与一次拷贝；
//! - **UDP GSO**：quinn 的 GSO 走 `quinn-udp` 的 Linux 路径
//!   （`UDP_SEGMENT`），Windows 上没有对应能力，而本项目主机侧
//!   （capture / encode）本来就是 Windows 专有 ⇒ 目标平台上不适用；
//! - **接收重组**：重组器是**同步纯状态机**（无 I/O、无 async），
//!   `VideoStream::try_next_frame` 是**非阻塞轮询**原语，
//!   由会话层（`beam-session`）的唯一取帧循环驱动 ⇒ 语义上仍是「单任务重组」；
//! - **FEC 解码**：在接收任务内同步完成。`rayon` 不是 workspace 依赖，
//!   引入它会违反「不新增依赖」；改用 `spawn_blocking` 需要把
//!   `Reassembler` / `try_next_frame` 改成 async，会溢出到
//!   `beam-session` / `beam-host` 等本 crate 无权修改的调用方。
//!
//! 结论：这三条登记为「**实现方式与文档描述不同**」，不是缺失。
//! 实测的 FEC 编解码耗时见 `benches/packetize.rs`（§3.2 性能预算的验证基准）。

use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::task::Poll;

use bytes::Bytes;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig, TransportConfig, VarInt};

use beam_pair::fingerprint::PeerVerifier;
use beam_pair::verifier::{rustls_client_verifier, rustls_verifier};
use beam_proto::{AssembledFrame, EncodedFrame};

use crate::config::{adaptive_fec_ratio, NetConfig};
use crate::datagram::{packetize_with_codec, Reassembler};
use crate::error::NetError;
use crate::fec::FecCodec;
use crate::jitter::JitterBuffer;

/// TLS 密码学错误的错误码区间（quinn：`0x100 | TLS alert`）。
///
/// 握手期出现这个区间的错误码 ⇒ TLS 层拒绝了握手，
/// 在本项目里最常见的原因就是**对端证书不被信任**（§3.2 `E2002`）。
const CRYPTO_ERROR_RANGE: std::ops::Range<u64> = 0x100..0x200;

/// TLS alert `no_application_protocol`（120）：ALPN 不匹配，**不是**证书问题。
const TLS_ALERT_NO_APPLICATION_PROTOCOL: u64 = 120;

/// 把 quinn 的握手错误映射成网络层错误码。
///
/// §3.2 的 `E2002`（证书被拒绝）必须有一条**真实产生路径**：
/// 对端证书被本端校验器拒绝时，rustls 会中止握手，
/// quinn 以 `CRYPTO_ERROR`（`0x100 | TLS alert`）结束连接。
/// 这里把它翻译成 `E2002`，而不是笼统的 `E2008` ——
/// 否则 UI 只能看到「网络 IO 失败」，拿不到「证书被拒」这个可操作信息。
///
/// - `TransportError`：本端在握手期直接报错（典型：本端拒绝了对端证书）；
/// - `ConnectionClosed`：对端中止了握手（典型：对端拒绝了本端证书）。
fn map_handshake_error(e: quinn::ConnectionError) -> NetError {
    let crypto_code = match &e {
        quinn::ConnectionError::TransportError(te) => Some(u64::from(te.code)),
        quinn::ConnectionError::ConnectionClosed(cc) => Some(u64::from(cc.error_code)),
        _ => None,
    };
    if let Some(code) = crypto_code {
        if CRYPTO_ERROR_RANGE.contains(&code) {
            if code & 0xFF == TLS_ALERT_NO_APPLICATION_PROTOCOL {
                return NetError::Io {
                    reason: format!("ALPN 不匹配: {e}"),
                };
            }
            return NetError::CertRejected {
                reason: format!("TLS 握手被拒(对端证书未通过校验): {e}"),
            };
        }
    }
    NetError::Io {
        reason: format!("QUIC 握手失败: {e}"),
    }
}

/// 链路估计（供发送路径自适应 FEC 比例用，§3.2 / §5）。
///
/// - `bitrate_bps`：当前码率估计；
/// - `loss_ppm`：当前丢包率估计（千分比）。
///
/// 用原子量而不是锁：`Conn` 是 `Clone` 的共享句柄，发送路径每帧都要读这两个值，
/// 无锁读比加锁更合适（§6「热路径禁止阻塞锁」）。
#[derive(Debug, Default)]
struct LinkEstimate {
    bitrate_bps: AtomicU32,
    loss_ppm: AtomicU32,
}

impl LinkEstimate {
    /// 读当前估计。
    fn get(&self) -> (u32, u32) {
        (
            self.bitrate_bps.load(Ordering::Relaxed),
            self.loss_ppm.load(Ordering::Relaxed),
        )
    }

    /// 写当前估计。
    fn set(&self, bitrate_bps: u32, loss_ppm: u32) {
        self.bitrate_bps.store(bitrate_bps, Ordering::Relaxed);
        self.loss_ppm.store(loss_ppm, Ordering::Relaxed);
    }
}

/// 视频发送器（§3.2）。
pub trait VideoSender {
    /// 发送一帧已编码的画面（内部负责分包与 FEC）。
    fn send_frame(&self, f: EncodedFrame) -> Result<(), NetError>;
}

/// 视频接收器（§3.2）。
pub trait VideoReceiver {
    /// 取下一帧完整画面。
    fn next_frame(&mut self) -> impl Future<Output = Result<AssembledFrame, NetError>> + Send;
}

/// QUIC 端点。
pub struct NetEndpoint {
    endpoint: Endpoint,
    cfg: NetConfig,
}

impl std::fmt::Debug for NetEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetEndpoint")
            .field("local_addr", &self.endpoint.local_addr().ok())
            .finish()
    }
}

impl NetEndpoint {
    /// 开始监听（§3.2 `NetEndpoint::listen`）。
    ///
    /// 需要 `cfg.identity` 提供本机证书与私钥；双向 TLS 校验由 `verifier` 完成。
    pub async fn listen(cfg: NetConfig, verifier: Arc<dyn PeerVerifier>) -> Result<Self, NetError> {
        let identity = cfg.identity.as_ref().ok_or(NetError::MissingIdentity)?;

        let mut tls = rustls::ServerConfig::builder()
            .with_client_cert_verifier(rustls_client_verifier(verifier))
            .with_single_cert(identity.cert_chain.clone(), identity.key.clone_key())
            .map_err(|e| NetError::CertRejected {
                reason: format!("装载本机证书失败: {e}"),
            })?;
        tls.alpn_protocols = vec![beam_proto::ALPN.to_vec()];

        let quic =
            QuicServerConfig::try_from(Arc::new(tls)).map_err(|e| NetError::CertRejected {
                reason: format!("构造 QUIC TLS 配置失败: {e}"),
            })?;
        let mut server = ServerConfig::with_crypto(Arc::new(quic));
        server.transport_config(Arc::new(transport_config(&cfg)));

        let bind = cfg.listen_addr;
        let endpoint = Endpoint::server(server, bind).map_err(|e| NetError::Io {
            reason: format!("监听 {bind} 失败: {e}"),
        })?;
        Ok(Self { endpoint, cfg })
    }

    /// 连接对端（§3.2 `NetEndpoint::connect`）。
    pub async fn connect(
        addr: SocketAddr,
        cfg: NetConfig,
        verifier: Arc<dyn PeerVerifier>,
    ) -> Result<Conn, NetError> {
        let identity = cfg.identity.as_ref().ok_or(NetError::MissingIdentity)?;

        let mut tls = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(rustls_verifier(verifier))
            .with_client_auth_cert(identity.cert_chain.clone(), identity.key.clone_key())
            .map_err(|e| NetError::CertRejected {
                reason: format!("装载本机证书失败: {e}"),
            })?;
        tls.alpn_protocols = vec![beam_proto::ALPN.to_vec()];

        let quic =
            QuicClientConfig::try_from(Arc::new(tls)).map_err(|e| NetError::CertRejected {
                reason: format!("构造 QUIC TLS 配置失败: {e}"),
            })?;
        let mut client = ClientConfig::new(Arc::new(quic));
        client.transport_config(Arc::new(transport_config(&cfg)));

        let bind = cfg.bind_addr;
        let mut endpoint = Endpoint::client(bind).map_err(|e| NetError::Io {
            reason: format!("创建客户端端点失败: {e}"),
        })?;
        endpoint.set_default_client_config(client);

        let server_name = cfg.server_name.clone();
        let connecting = endpoint
            .connect(addr, &server_name)
            .map_err(|e| NetError::Io {
                reason: format!("发起连接失败: {e}"),
            })?;
        let timeout = cfg.connect_timeout;
        let connection = tokio::time::timeout(timeout, connecting)
            .await
            .map_err(|_| NetError::ConnectTimeout {
                addr: addr.to_string(),
            })?
            // 握手期被拒 ⇒ E2002（证书被拒绝）而不是笼统的 E2008。
            .map_err(map_handshake_error)?;

        Ok(Conn {
            connection,
            cfg,
            link: Arc::new(LinkEstimate::default()),
            fec: Arc::new(FecCodec::new()),
        })
    }

    /// 本端地址。
    pub fn local_addr(&self) -> Result<SocketAddr, NetError> {
        self.endpoint.local_addr().map_err(|e| NetError::Io {
            reason: format!("读取本地地址失败: {e}"),
        })
    }

    /// 等待下一个连接。
    pub async fn accept(&self) -> Result<Conn, NetError> {
        let incoming = self.endpoint.accept().await.ok_or_else(|| NetError::Io {
            reason: "端点已关闭".to_string(),
        })?;
        // 对端在握手期中止（典型：对端拒绝了本端证书）⇒ E2002。
        let connection = incoming.await.map_err(map_handshake_error)?;
        Ok(Conn {
            connection,
            cfg: self.cfg.clone(),
            link: Arc::new(LinkEstimate::default()),
            fec: Arc::new(FecCodec::new()),
        })
    }

    /// 关闭端点。
    pub fn close(&self) {
        self.endpoint.close(VarInt::from_u32(0), b"bye");
    }
}

fn transport_config(_cfg: &NetConfig) -> TransportConfig {
    let mut transport = TransportConfig::default();
    // 打开 datagram 收发缓冲（§5 视频 / 音频走 QUIC datagram）。
    transport.datagram_receive_buffer_size(Some(1024 * 1024));
    transport.datagram_send_buffer_size(1024 * 1024);
    // §3.2 `max_datagram=1200`：路径 MTU 留出 QUIC 帧头开销。
    transport.initial_mtu(1400);
    transport.min_mtu(1200);
    transport.max_concurrent_bidi_streams(VarInt::from_u32(8));
    transport.max_concurrent_uni_streams(VarInt::from_u32(8));
    transport
}

/// 一条已建立的 QUIC 连接。
#[derive(Debug, Clone)]
pub struct Conn {
    connection: Connection,
    cfg: NetConfig,
    /// 链路估计（码率 / 丢包率），供发送路径自适应 FEC 比例。
    link: Arc<LinkEstimate>,
    /// **跨帧复用**的 FEC 编解码器（缓存 RS 矩阵，§3.2 性能预算）。
    ///
    /// ## 为什么必须挂在连接上
    ///
    /// `ReedSolomon::new(k, m)` 要在 GF(2⁸) 上求逆一个 `k×k` 矩阵（O(k³)）：
    /// 实测 T1 档位（`k=132`）单次 **4.894 ms**，而整帧「分包 + FEC 编码」
    /// 只要 6.635 ms —— 也就是说 **74% 的时间花在重建矩阵上**，
    /// 真正的 GF 乘加只有 0.773 ms。
    ///
    /// 挂在连接上之后，矩阵**每条连接只建一次**（且只在真的需要 FEC 时才建），
    /// 之后每帧只付 GF 乘加的钱。
    /// `FecCodec` 内部的缓存表本身就是 `Arc<Mutex<..>>`，
    /// 因此这里用 `Arc` 共享即可，`Conn` 的克隆体也共享同一份矩阵。
    fec: Arc<FecCodec>,
}

impl Conn {
    /// 打开一条双向流（控制通道 stream 0）。
    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), NetError> {
        let (send, recv) = self.connection.open_bi().await.map_err(|e| NetError::Io {
            reason: format!("打开双向流失败: {e}"),
        })?;
        Ok((send, recv))
    }

    /// 打开一条单向流（输入通道）。
    pub async fn open_uni(&self) -> Result<quinn::SendStream, NetError> {
        let send = self.connection.open_uni().await.map_err(|e| NetError::Io {
            reason: format!("打开单向流失败: {e}"),
        })?;
        Ok(send)
    }

    /// 接受一条双向流。
    pub async fn accept_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), NetError> {
        let (send, recv) = self
            .connection
            .accept_bi()
            .await
            .map_err(|e| NetError::Io {
                reason: format!("接受双向流失败: {e}"),
            })?;
        Ok((send, recv))
    }

    /// 接受一条单向流（输入通道，§5 输入 stream 客户端 → 主机）。
    ///
    /// 与 [`Conn::accept_bi`] 对称：QUIC 的单向流只有在收到对端数据后才对本端可见，
    /// 因此本函数会等到客户端在输入流上写入第一条数据才返回。
    pub async fn accept_uni(&self) -> Result<quinn::RecvStream, NetError> {
        let recv = self
            .connection
            .accept_uni()
            .await
            .map_err(|e| NetError::Io {
                reason: format!("接受单向流失败: {e}"),
            })?;
        Ok(recv)
    }

    /// 发送一个数据报。
    pub fn send_datagram(&self, data: &[u8]) -> Result<(), NetError> {
        self.connection
            .send_datagram(Bytes::copy_from_slice(data))
            .map_err(|e| match e {
                quinn::SendDatagramError::UnsupportedByPeer => NetError::DatagramUnsupported,
                quinn::SendDatagramError::Disabled => NetError::DatagramUnsupported,
                quinn::SendDatagramError::TooLarge => NetError::ReassemblyFailed {
                    reason: "数据报超过对端 MTU".to_string(),
                },
                quinn::SendDatagramError::ConnectionLost(_) => NetError::Io {
                    reason: "连接已断开".to_string(),
                },
            })
    }

    /// 读取一个数据报。
    pub async fn read_datagram(&self) -> Result<Vec<u8>, NetError> {
        self.connection
            .read_datagram()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| NetError::Io {
                reason: format!("读取数据报失败: {e}"),
            })
    }

    /// 非阻塞地读取一个数据报；暂时没有数据时返回 `Ok(None)`。
    ///
    /// 与 [`Conn::read_datagram`] 的区别是**不等待网络**：
    /// 只把已经到达并缓存在 QUIC 接收缓冲里的数据报取走，
    /// 取不到就立刻返回 `Ok(None)`，供轮询式取帧使用。
    pub fn try_read_datagram(&self) -> Result<Option<Vec<u8>>, NetError> {
        let mut fut = Box::pin(self.connection.read_datagram());
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(bytes)) => Ok(Some(bytes.to_vec())),
            Poll::Ready(Err(e)) => Err(NetError::Io {
                reason: format!("读取数据报失败: {e}"),
            }),
            // 还没有数据报到达；quinn 的数据报缓存在连接状态里，不会丢。
            Poll::Pending => Ok(None),
        }
    }

    /// 生成视频接收流（携带重组器与抖动缓冲）。
    pub fn video_stream(&self) -> VideoStream {
        VideoStream {
            conn: self.clone(),
            reassembler: Reassembler::new(),
            jitter: JitterBuffer::new(self.cfg.jitter_depth()),
            expired: Vec::new(),
        }
    }

    /// 关闭连接。
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.connection.close(VarInt::from_u32(code), reason);
    }

    /// 连接统计。
    pub fn stats(&self) -> quinn::ConnectionStats {
        self.connection.stats()
    }

    /// 内部配置（只读）。
    pub fn config(&self) -> &NetConfig {
        &self.cfg
    }

    /// 更新链路估计（码率、丢包率），发送路径据此自适应 FEC 比例。
    ///
    /// §3.2 配置段：「`fec_ratio`（默认 0.2，**码率 >150 Mbps 时下调到 0.1**）」；
    /// §5 进一步规定「丢包 >2% 升至 0.3」。
    ///
    /// 会话层（`beam-session`）拿到 [`crate::Feedback`] 并算出码率 / 丢包率后
    /// 调用本方法，发送路径下一次发帧时就会用上自适应后的比例。
    ///
    /// 默认值是 `(0, 0)`：既不触发高码率下调，也不触发高丢包上调，
    /// FEC 比例就是配置里的 `fec_ratio`。
    pub fn update_link_estimate(&self, bitrate_bps: u32, loss_ppm: u32) {
        self.link.set(bitrate_bps, loss_ppm);
    }

    /// 当前链路估计（码率、丢包率）。
    pub fn link_estimate(&self) -> (u32, u32) {
        self.link.get()
    }

    /// 连接当前能承载的**单个数据报**上限（字节）。
    ///
    /// `None` 表示对端不支持 datagram（此时发送会报 `E2003`）。
    pub fn max_datagram_limit(&self) -> Option<usize> {
        self.connection.max_datagram_size()
    }

    /// 发送路径实际使用的最大数据报长度（字节）。
    ///
    /// 取 [`NetConfig::max_datagram`] 与连接当前上限的**较小者**。
    ///
    /// ## 为什么必须夹一刀
    ///
    /// §3.2 的 `max_datagram=1200` 是「我们自己希望的数据报大小」，
    /// 但 QUIC 能承载的数据报还要扣掉 1-RTT 包头与 AEAD tag（约 32 B）。
    /// 路径 MTU 会变：quinn 的黑洞检测（black hole detection）在探测包丢失时
    /// 会把 MTU 降到 `min_mtu`（1200），此时**可承载的数据报只剩约 1168 B**，
    /// 一个 1200 B 的数据报直接发不出去（`SendDatagramError::TooLarge`）。
    ///
    /// 也就是说：**只要路径抖动一次，整条视频链路就会彻底发不出帧**。
    /// 夹一刀之后，同一帧会被切成更小的分片照常发出去，接收端照常重组
    /// （组数与组内分片号都在帧头里，接收端不依赖发送端的 `max_datagram`）。
    pub fn effective_max_datagram(&self) -> usize {
        let configured = self.cfg.max_datagram;
        match self.connection.max_datagram_size() {
            Some(limit) => configured.min(limit),
            // 对端不支持 datagram：保留配置值，发送时会报 E2003。
            None => configured,
        }
    }

    /// 发送路径当前实际使用的 FEC 比例。
    ///
    /// - `fec` feature 关闭（MVP）：恒为 `0.0` —— 不产生任何冗余分片；
    /// - `fec` feature 打开（v1）：按 [`adaptive_fec_ratio`] 的规则
    ///   （码率 >150 Mbps → 0.1；丢包 >2% → 0.3；其余取配置的 `fec_ratio`）。
    pub fn current_fec_ratio(&self) -> f32 {
        let base = self.cfg.effective_fec_ratio();
        if base <= 0.0 {
            // MVP 构建：`fec_ratio` 配置项保留但不生效，
            // 绝不能因为「丢包高」就凭空开始产生冗余片。
            return 0.0;
        }
        let (bitrate_bps, loss_ppm) = self.link.get();
        adaptive_fec_ratio(base, bitrate_bps, loss_ppm)
    }
}

impl VideoSender for Conn {
    fn send_frame(&self, f: EncodedFrame) -> Result<(), NetError> {
        // FEC 比例：MVP（未启用 `fec` feature）恒为 0，不产生冗余分片；
        // v1 按 §5 的自适应规则（码率 / 丢包率）调整。
        //
        // 数据报大小按连接当前能承载的上限夹一刀（见
        // [`Conn::effective_max_datagram`]）：路径 MTU 掉下来时，
        // 1200 B 的数据报会直接发不出去。
        //
        // FEC 编解码器取自连接（跨帧复用），让 RS 矩阵只建一次 ——
        // 见 [`Conn::fec`] 的注释。
        let packets = packetize_with_codec(
            &f,
            self.current_fec_ratio(),
            self.effective_max_datagram(),
            false,
            &self.fec,
        )?;
        for p in packets {
            self.send_datagram(&p.to_datagram())?;
        }
        Ok(())
    }
}

/// 视频接收流：读数据报并重组出完整帧。
#[derive(Debug)]
pub struct VideoStream {
    conn: Conn,
    reassembler: Reassembler,
    jitter: JitterBuffer,
    /// 因超时被丢弃的帧（每个对应一条 `E2004`）。
    expired: Vec<NetError>,
}

impl VideoStream {
    /// 重组器（用于统计与超时处理）。
    pub fn reassembler(&self) -> &Reassembler {
        &self.reassembler
    }

    /// 抖动缓冲（§3.2 `jitter_frames`）。
    pub fn jitter(&self) -> &JitterBuffer {
        &self.jitter
    }

    /// 非阻塞地取下一帧画面；暂无完整帧时返回 `Ok(None)`。
    ///
    /// 语义：
    ///
    /// 1. **不完整帧会被保留**：收到的分片先进入重组器，等后续分片补齐；
    ///    `try_next_frame` 返回 `Ok(None)` 只表示「此刻还没有完整帧」，
    ///    不表示数据丢了。
    /// 2. **过期帧会被丢弃**：每次调用先清理超过 `frame_timeout_us`
    ///    仍未补齐的帧（超时从第一片到达起算，见 [`Reassembler::expire`]），
    ///    被丢弃的帧号通过 [`VideoStream::take_expired`] 取出，
    ///    调用方按 §3.2 记为 **`E2004`**（`NetError::FrameExpired`）。
    /// 3. 完整帧先过抖动缓冲（§3.2 `jitter_frames`）再交出去；
    ///    深度为 0 时不增加延迟。
    ///
    /// 本方法只消费**已经到达**的数据报，不会等待网络。
    pub fn try_next_frame(&mut self) -> Result<Option<AssembledFrame>, NetError> {
        // 1) 清理超时帧（本地单调时钟口径），丢弃的帧记为 E2004。
        for frame_id in self.reassembler.expire_stale() {
            self.expired.push(NetError::FrameExpired { frame_id });
        }
        // 2) 非阻塞地读走已经到达的数据报并喂给重组器。
        while let Some(datagram) = self.conn.try_read_datagram()? {
            if let Some(frame) = self.reassembler.push_datagram(&datagram)? {
                self.jitter.push(frame, crate::now_us());
            }
        }
        // 3) 经抖动缓冲放行一帧。
        Ok(self.jitter.pop(crate::now_us()))
    }

    /// 取走因超时被丢弃的帧（每条都是 `E2004`：`NetError::FrameExpired`）。
    pub fn take_expired(&mut self) -> Vec<NetError> {
        std::mem::take(&mut self.expired)
    }
}

impl VideoReceiver for VideoStream {
    async fn next_frame(&mut self) -> Result<AssembledFrame, NetError> {
        loop {
            // 抖动缓冲里已经有可以放行的帧就先放行。
            if let Some(frame) = self.jitter.pop(crate::now_us()) {
                return Ok(frame);
            }
            let datagram = match self.jitter.next_release_in(crate::now_us()) {
                // 缓冲里没有待释放的帧：阻塞等下一个数据报。
                None => self.conn.read_datagram().await?,
                // 有帧被压着：最多等到它可以释放的时刻。
                Some(wait) => match tokio::time::timeout(wait, self.conn.read_datagram()).await {
                    Ok(result) => result?,
                    Err(_) => continue,
                },
            };
            if let Some(frame) = self.reassembler.push_datagram(&datagram)? {
                self.jitter.push(frame, crate::now_us());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use beam_pair::{Identity, MemoryKeyStore, TrustStore, TrustedPeer};

    fn identity() -> crate::config::TlsIdentity {
        let id = Identity::load_or_create(&MemoryKeyStore::new()).expect("应当能生成身份");
        crate::config::TlsIdentity {
            cert_chain: id.certificate_chain(),
            key: id.private_key(),
        }
    }

    #[tokio::test]
    async fn listen_without_identity_is_e2007() {
        let cfg = NetConfig::default();
        let err = NetEndpoint::listen(cfg, Arc::new(TrustStore::new()))
            .await
            .expect_err("缺少证书必须失败");
        assert_eq!(err.code(), 2007);
    }

    #[tokio::test]
    async fn connect_without_identity_is_e2007() {
        let cfg = NetConfig::default();
        let addr = "127.0.0.1:1".parse().expect("合法地址");
        let err = NetEndpoint::connect(addr, cfg, Arc::new(TrustStore::new()))
            .await
            .expect_err("缺少证书必须失败");
        assert_eq!(err.code(), 2007);
    }

    #[tokio::test]
    async fn listen_binds_local_address() {
        let cfg = NetConfig {
            identity: Some(identity()),
            listen_addr: "127.0.0.1:0".parse().expect("合法地址"),
            ..NetConfig::default()
        };
        let endpoint = NetEndpoint::listen(cfg, Arc::new(TrustStore::new()))
            .await
            .expect("应当能监听");
        let addr = endpoint.local_addr().expect("应当能读到地址");
        assert!(addr.port() > 0);
    }

    // ---- 本机回环：真实 QUIC 端点 + 自签证书 + 互相信任库 ----

    /// 起一对真实连接，返回（客户端, 主机端）。
    async fn connected_pair() -> (Conn, Conn) {
        let server_id = Identity::generate().expect("应当能生成服务端身份");
        let client_id = Identity::generate().expect("应当能生成客户端身份");

        let server_cfg = NetConfig {
            identity: Some(crate::config::TlsIdentity {
                cert_chain: server_id.certificate_chain(),
                key: server_id.private_key(),
            }),
            listen_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
            ..NetConfig::default()
        };

        let mut server_store = TrustStore::new();
        server_store.upsert(TrustedPeer::new(
            client_id.fingerprint(),
            "client",
            "windows",
            0,
        ));

        let server = NetEndpoint::listen(server_cfg, Arc::new(server_store))
            .await
            .expect("应当能监听");
        let addr = server.local_addr().expect("应当能读到地址");

        let client_cfg = NetConfig {
            identity: Some(crate::config::TlsIdentity {
                cert_chain: client_id.certificate_chain(),
                key: client_id.private_key(),
            }),
            bind_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
            ..NetConfig::default()
        };

        let mut client_store = TrustStore::new();
        client_store.upsert(TrustedPeer::new(
            server_id.fingerprint(),
            "server",
            "windows",
            0,
        ));

        // 主机侧的 accept 与客户端的 connect 必须并发进行：
        // QUIC 的服务端握手要靠 accept 被轮询才能推进。
        let (server_conn, client) = tokio::join!(
            server.accept(),
            NetEndpoint::connect(addr, client_cfg, Arc::new(client_store))
        );
        let server_conn = server_conn.expect("应当能接受连接");
        let client = client.expect("应当能连上");
        (client, server_conn)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accept_uni_receives_host_side_input_stream() {
        let (client, server) = connected_pair().await;

        // 客户端开一条单向流（§5 输入 stream），写完就关闭发送方向。
        let mut send = client.open_uni().await.expect("应当能打开单向流");
        send.write_all(b"beam-input")
            .await
            .expect("应当能写入单向流");
        send.finish().expect("应当能关闭发送方向");

        // 主机侧用 accept_uni 收。
        let mut recv = server.accept_uni().await.expect("应当能接受单向流");
        let payload = recv.read_to_end(4096).await.expect("应当能读完单向流");
        assert_eq!(payload, b"beam-input");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn try_next_frame_returns_frame_without_blocking() {
        let (client, server) = connected_pair().await;

        let frame = EncodedFrame {
            frame_id: 7,
            pts_us: 100,
            capture_ts_us: 200,
            is_key: true,
            slices: vec![Bytes::from_static(b"encoded-picture")],
        };
        client.send_frame(frame).expect("应当能发出画面");

        let mut stream = server.video_stream();
        let mut got = None;
        for _ in 0..200 {
            if let Some(f) = stream.try_next_frame().expect("取帧不应报错") {
                got = Some(f);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let got = got.expect("应当取到完整帧");
        assert_eq!(got.frame_id, 7);
        assert_eq!(got.capture_ts_us, 200);
        assert!(got.is_key);
        assert_eq!(got.payload.as_ref(), b"encoded-picture");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn try_next_frame_returns_none_when_nothing_arrived() {
        let (_client, server) = connected_pair().await;
        let mut stream = server.video_stream();
        assert!(
            stream.try_next_frame().expect("取帧不应报错").is_none(),
            "没有数据时应当返回 Ok(None)"
        );
    }

    /// 发送路径的数据报大小必须**不超过连接当前能承载的上限**。
    ///
    /// 这是回归测试：路径 MTU 掉下来（quinn 的黑洞检测会把 MTU 降到
    /// `min_mtu = 1200`，此时可承载的数据报只剩约 1168 B）时，
    /// 固定用 1200 B 的数据报会直接报「数据报超过对端 MTU」，
    /// **整条视频链路彻底发不出帧**。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_path_never_exceeds_the_connection_datagram_limit() {
        let (client, _server) = connected_pair().await;
        let limit = client.max_datagram_limit().expect("应当支持 datagram");
        assert!(
            client.effective_max_datagram() <= limit,
            "发送用的数据报大小({})不得超过连接上限({limit})",
            client.effective_max_datagram()
        );
        assert!(client.effective_max_datagram() <= client.config().max_datagram);

        // 一帧 200 KB（切成上百个数据报）必须发得出去。
        let frame = EncodedFrame {
            frame_id: 9,
            pts_us: 0,
            capture_ts_us: 0,
            is_key: true,
            slices: vec![Bytes::from(vec![0x33u8; 200_000])],
        };
        client.send_frame(frame).expect("大帧应当发得出去");
    }

    // ---- N-16：E2002 证书拒绝的真实产生路径 ----

    /// 客户端不信任服务端证书 ⇒ 握手被拒 ⇒ **E2002**（而不是笼统的 E2008）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejected_peer_certificate_reports_e2002() {
        let server_id = Identity::generate().expect("应当能生成服务端身份");
        let client_id = Identity::generate().expect("应当能生成客户端身份");

        let server_cfg = NetConfig {
            identity: Some(crate::config::TlsIdentity {
                cert_chain: server_id.certificate_chain(),
                key: server_id.private_key(),
            }),
            listen_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
            ..NetConfig::default()
        };
        let server = NetEndpoint::listen(server_cfg, Arc::new(TrustStore::new()))
            .await
            .expect("应当能监听");
        let addr = server.local_addr().expect("应当能读到地址");

        let client_cfg = NetConfig {
            identity: Some(crate::config::TlsIdentity {
                cert_chain: client_id.certificate_chain(),
                key: client_id.private_key(),
            }),
            bind_addr: "127.0.0.1:0".parse().expect("合法回环地址"),
            ..NetConfig::default()
        };
        // 客户端的信任库是空的 ⇒ 服务端证书被拒。
        let (_server_side, client_side) = tokio::join!(
            server.accept(),
            NetEndpoint::connect(addr, client_cfg, Arc::new(TrustStore::new()))
        );
        let err = client_side.expect_err("证书被拒时连接必须失败");
        assert_eq!(err.code(), 2002, "对端证书被拒必须报 E2002,实际: {err}");
    }

    // ---- N-20：`fec_ratio` 的自适应规则必须真正进入发送路径 ----

    /// 链路估计为空时，FEC 比例就是配置里的 `fec_ratio`。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fec_ratio_defaults_to_configured_value() {
        let (client, _server) = connected_pair().await;
        assert_eq!(client.link_estimate(), (0, 0));
        if crate::fec::FEC_ENABLED {
            assert_eq!(client.current_fec_ratio(), 0.2);
        } else {
            // MVP：配置项保留但不生效。
            assert_eq!(client.current_fec_ratio(), 0.0);
        }
    }

    /// 码率 >150 Mbps ⇒ 比例下调到 0.1；丢包 >2% ⇒ 比例升到 0.3。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fec_ratio_follows_the_adaptive_rule() {
        let (client, _server) = connected_pair().await;
        if !crate::fec::FEC_ENABLED {
            // MVP 构建：`fec_ratio` 配置项保留但不生效，
            // 无论链路估计怎么变都不会产生冗余片。
            client.update_link_estimate(200_000_000, 5_000);
            assert_eq!(client.current_fec_ratio(), 0.0);
            return;
        }
        client.update_link_estimate(200_000_000, 0);
        assert_eq!(
            client.current_fec_ratio(),
            0.1,
            "码率超 150 Mbps 应下调到 0.1"
        );
        client.update_link_estimate(40_000_000, 5_000);
        assert_eq!(client.current_fec_ratio(), 0.3, "丢包超 2% 应升到 0.3");
        client.update_link_estimate(40_000_000, 0);
        assert_eq!(client.current_fec_ratio(), 0.2, "其余情况取配置的 0.2");
    }

    /// 自适应比例必须**真的**改变发出去的数据报构成（冗余片数量）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adaptive_fec_ratio_changes_the_datagrams_on_the_wire() {
        let (client, server) = connected_pair().await;
        if !crate::fec::FEC_ENABLED {
            return;
        }
        // 200 KB 的帧 ⇒ 单组 k = ceil(200004/1184) = 169。
        let payload = vec![0x5Au8; 200_000];
        let frame = EncodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            is_key: true,
            slices: vec![Bytes::from(payload)],
        };

        /// 收走连接上已经到达的全部数据报，返回其中的 FEC 冗余片数量。
        async fn drain_parity(server: &Conn) -> usize {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut parity = 0usize;
            let mut last_seen = std::time::Instant::now();
            while std::time::Instant::now() < deadline {
                let mut got_any = false;
                while let Some(dg) = server.try_read_datagram().expect("读数据报不应报错") {
                    got_any = true;
                    let h = beam_proto::VideoPktHeader::decode(&dg).expect("帧头应当能解");
                    if matches!(h.kind, beam_proto::FrameKind::Fec) {
                        parity += 1;
                    }
                }
                if got_any {
                    last_seen = std::time::Instant::now();
                } else if last_seen.elapsed() > Duration::from_millis(300) {
                    // 连续 300 ms 没有新数据报 ⇒ 收完了。
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            parity
        }

        // 默认（ratio 0.2）时的冗余片数量。
        client.send_frame(frame.clone()).expect("应当能发出画面");
        let parity_default = drain_parity(&server).await;
        assert!(parity_default > 0, "默认应当产生冗余片");

        // 码率超 150 Mbps ⇒ ratio 0.1 ⇒ 冗余片应当变少。
        client.update_link_estimate(200_000_000, 0);
        client.send_frame(frame.clone()).expect("应当能发出画面");
        let parity_high = drain_parity(&server).await;
        assert!(
            parity_high < parity_default,
            "高码率下冗余片应当变少({parity_high} < {parity_default})"
        );
    }
}
