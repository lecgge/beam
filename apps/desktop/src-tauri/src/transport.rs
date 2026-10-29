//! 真实 QUIC 传输(§3.2)。
//!
//! 用 `beam-net` 的 `NetEndpoint::connect` 建立一条真实的 QUIC/TLS 连接:
//! - TLS 身份来自 `beam-pair` 的本机自签证书(§3.3);
//! - 对端证书由 `beam-pair` 的信任库校验(§3.3「证书指纹固定」)。
//!
//! 边界:QUIC 需要 tokio reactor,所以这里必须是 `async fn`,
//! 由 `tauri::async_runtime::spawn` 驱动;不能用 `futures::executor::block_on`,
//! 那不会驱动 reactor,连接会挂住。

#![cfg(feature = "beam-crates")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use beam_config::ResolvedConfig;
use beam_net::{NetConfig, NetEndpoint, TlsIdentity};
use beam_pair::{Identity, PeerVerifier, TrustStore};

use crate::error::BeamError;

/// 建立 QUIC 连接所需的参数。
pub struct TransportRequest {
    /// 对端地址。
    pub addr: SocketAddr,
    /// 解析后的配置(§3.14)→ 网络参数。
    pub resolved: ResolvedConfig,
    /// 本机 TLS 身份(§3.3)。
    pub identity: Identity,
    /// 信任库(§3.3):只有已配对主机的证书指纹才会通过校验。
    pub trust: TrustStore,
    /// TLS server name。
    pub server_name: String,
}

/// 建立真实 QUIC 连接。
///
/// `Ok(())` 代表 TLS 握手已经完成,即传输层可达。
/// 画面数据不在这里传输——正式会话的帧传输由 `beam-session` 的
/// `TransportFactory` 负责(当前 crate 侧尚未提供可用封装),
/// 因此握手成功后立即关闭这条探测连接。
pub async fn connect_peer(request: TransportRequest) -> Result<(), BeamError> {
    let mut net_config = NetConfig::default();
    net_config.fec_ratio = request.resolved.network.fec_ratio;
    net_config.max_datagram = request.resolved.network.max_datagram;
    net_config.jitter_frames = request.resolved.network.jitter_frames;
    net_config.bitrate_min = request.resolved.network.bitrate_min;
    net_config.bitrate_max = request.resolved.network.bitrate_max;
    net_config.connect_timeout = crate::service::CONNECT_TIMEOUT;
    net_config.server_name = request.server_name.clone();
    net_config.bind_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);

    // §3.2:两端都要有 TLS 身份,否则 E2007。
    net_config.identity = Some(TlsIdentity {
        cert_chain: request.identity.certificate_chain(),
        key: request.identity.private_key(),
    });

    // §3.3:证书校验交给信任库,网络层不需要理解配对语义。
    let verifier: Arc<dyn PeerVerifier> = Arc::new(request.trust);

    let conn = NetEndpoint::connect(request.addr, net_config, verifier)
        .await
        .map_err(|err| {
            BeamError::retryable(
                &err.code_str(),
                format!("QUIC 握手失败 {}: {}", err.code_str(), err.text()),
            )
        })?;

    // 握手成功:传输层可达。这条连接只用于探测,随后释放。
    conn.close(0, b"probe");
    Ok(())
}
