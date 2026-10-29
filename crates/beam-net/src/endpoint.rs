//! QUIC 端点与连接（§3.2 `NetEndpoint::listen/connect`、`VideoSender` / `VideoReceiver`）。
//!
//! §5：ALPN `beam/1`；控制 stream 0 双向可靠；视频 / 音频走 QUIC datagram。
//! §4：TLS 校验回调由 pair 提供（`PeerVerifier`），网络层不理解信任库语义。

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::Poll;

use bytes::Bytes;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig, TransportConfig, VarInt};

use beam_pair::fingerprint::PeerVerifier;
use beam_pair::verifier::{rustls_client_verifier, rustls_verifier};
use beam_proto::{AssembledFrame, EncodedFrame};

use crate::config::NetConfig;
use crate::datagram::{packetize, Reassembler};
use crate::error::NetError;
use crate::jitter::JitterBuffer;

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
            .map_err(|e| NetError::Io {
                reason: format!("QUIC 握手失败: {e}"),
            })?;

        Ok(Conn { connection, cfg })
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
        let connection = incoming.await.map_err(|e| NetError::Io {
            reason: format!("接受连接失败: {e}"),
        })?;
        Ok(Conn {
            connection,
            cfg: self.cfg.clone(),
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
}

impl VideoSender for Conn {
    fn send_frame(&self, f: EncodedFrame) -> Result<(), NetError> {
        // `effective_fec_ratio`：`fec` feature 关闭（MVP）时恒为 0，不产生冗余分片。
        let packets = packetize(
            &f,
            self.cfg.effective_fec_ratio(),
            self.cfg.max_datagram,
            false,
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
}
