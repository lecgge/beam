//! QUIC 端点与连接（§3.2 `NetEndpoint::listen/connect`、`VideoSender` / `VideoReceiver`）。
//!
//! §5：ALPN `beam/1`；控制 stream 0 双向可靠；视频 / 音频走 QUIC datagram。
//! §4：TLS 校验回调由 pair 提供（`PeerVerifier`），网络层不理解信任库语义。

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig, TransportConfig, VarInt};

use beam_pair::fingerprint::PeerVerifier;
use beam_pair::verifier::{rustls_client_verifier, rustls_verifier};
use beam_proto::{AssembledFrame, EncodedFrame};

use crate::config::NetConfig;
use crate::datagram::{packetize, Reassembler};
use crate::error::NetError;

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

    /// 生成视频接收流（携带重组器）。
    pub fn video_stream(&self) -> VideoStream {
        VideoStream {
            conn: self.clone(),
            reassembler: Reassembler::new(),
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
        let packets = packetize(&f, self.cfg.fec_ratio, self.cfg.max_datagram, false)?;
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
}

impl VideoStream {
    /// 重组器（用于统计与超时处理）。
    pub fn reassembler(&self) -> &Reassembler {
        &self.reassembler
    }
}

impl VideoReceiver for VideoStream {
    fn next_frame(&mut self) -> impl Future<Output = Result<AssembledFrame, NetError>> + Send {
        async move {
            loop {
                let datagram = self.conn.read_datagram().await?;
                if let Some(frame) = self.reassembler.push_datagram(&datagram)? {
                    return Ok(frame);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_pair::{Identity, MemoryKeyStore, TrustStore};

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
        let mut cfg = NetConfig::default();
        cfg.identity = Some(identity());
        cfg.listen_addr = "127.0.0.1:0".parse().expect("合法地址");
        let endpoint = NetEndpoint::listen(cfg, Arc::new(TrustStore::new()))
            .await
            .expect("应当能监听");
        let addr = endpoint.local_addr().expect("应当能读到地址");
        assert!(addr.port() > 0);
    }
}
