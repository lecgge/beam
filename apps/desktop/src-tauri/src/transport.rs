//! 真实 QUIC 传输(§3.2)。
//!
//! 用 `beam-net` 的 `NetEndpoint::connect` 建立一条真实的 QUIC/TLS 连接:
//! - TLS 身份来自 `beam-pair` 的本机自签证书(§3.3);
//! - 对端证书由 `beam-pair` 的信任库校验(§3.3「证书指纹固定」)。
//!
//! 边界:QUIC 需要 tokio reactor,所以这里必须是 `async fn`,
//! 由 `tauri::async_runtime::spawn` 驱动;不能用 `futures::executor::block_on`,
//! 那不会驱动 reactor,连接会挂住。
//!
//! 本模块整体由 `main.rs` 里的 `#[cfg(feature = "beam-crates")] mod transport;`
//! 门控 —— 这里**不再**写一遍内层 `#![cfg]`,否则是重复属性(同一个 cfg
//! 挂在同一个模块上两次)。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use beam_config::ResolvedConfig;
use beam_net::{NetConfig, NetEndpoint, TlsIdentity};
use beam_pair::{Fingerprint, Identity, PeerVerifier, TrustStore};

use crate::error::BeamError;

/// §5 的 TLS server name。
///
/// 所有 Beam 身份的证书 SAN 都是 `beam.local`(`beam_pair::Identity` 生成时写死),
/// 因此**不能**把 mDNS 里的展示名(可能是中文)当 server name 用:
/// 那会让 TLS 参数校验直接失败。
pub const TLS_SERVER_NAME: &str = "beam.local";

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
}

/// 采集对端证书指纹所需的参数。
pub struct ProbeRequest {
    /// 对端地址。
    pub addr: SocketAddr,
    /// 解析后的配置(§3.14)→ 网络参数。
    pub resolved: ResolvedConfig,
    /// 本机 TLS 身份(§3.3)。
    pub identity: Identity,
}

/// 监听所需的参数(§3.2)。
pub struct ListenRequest {
    /// 解析后的配置(§3.14)→ 网络参数。
    pub resolved: ResolvedConfig,
    /// 本机 TLS 身份(§3.3)。
    pub identity: Identity,
    /// 监听地址(§3.2):`0.0.0.0:<port>`。
    pub bind_addr: SocketAddr,
}

/// 由解析后的配置组装网络参数(§3.14 → §3.2)。
fn net_config(resolved: &ResolvedConfig, identity: &Identity) -> NetConfig {
    NetConfig {
        fec_ratio: resolved.network.fec_ratio,
        max_datagram: resolved.network.max_datagram,
        jitter_frames: resolved.network.jitter_frames,
        bitrate_min: resolved.network.bitrate_min,
        bitrate_max: resolved.network.bitrate_max,
        connect_timeout: crate::service::CONNECT_TIMEOUT,
        server_name: TLS_SERVER_NAME.to_string(),
        bind_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
        // §3.2:两端都要有 TLS 身份,否则 E2007。
        identity: Some(TlsIdentity {
            cert_chain: identity.certificate_chain(),
            key: identity.private_key(),
        }),
        ..NetConfig::default()
    }
}

/// 建立真实 QUIC 连接。
///
/// 返回的连接**不关闭**:调用方(客户端会话)接着在上面开控制流 / 输入流。
/// 画面走 datagram,由 `beam-client` 的传输适配器收发。
pub async fn connect_peer(request: TransportRequest) -> Result<beam_net::Conn, BeamError> {
    let net_config = net_config(&request.resolved, &request.identity);
    // §3.3:证书校验交给信任库,网络层不需要理解配对语义。
    let verifier: Arc<dyn PeerVerifier> = Arc::new(request.trust);
    NetEndpoint::connect(request.addr, net_config, verifier)
        .await
        .map_err(|err| {
            // 码直接取 `beam-net` 的 `code_str()`(E2xxx 段),前端按码查文案表。
            //
            // 可重试性也按 crate 自己的分类走(`NetError::retryable`):
            // 证书被拒(E2002)、缺本机证书(E2007)重试也没用,
            // 必须提示用户改配置,而不是弹一个「可重试」。
            BeamError::new(
                &err.code_str(),
                format!("QUIC 握手失败 {}: {}", err.code_str(), err.text()),
                err.retryable(),
            )
        })
}

/// 采集对端证书指纹的校验器。
///
/// 握手**必然失败**:校验器一律拒绝,只把对端的完整指纹记下来。
///
/// 为什么需要它:§3.4 的 mDNS TXT 只广播指纹**前 8 位**,
/// 而信任库(§3.3)需要完整指纹才能做 TLS 固定。
/// 采集本身不等于信任——真正的信任必须由用户在弹窗里确认后写入信任库。
#[derive(Debug, Clone, Default)]
pub struct FingerprintProbe {
    seen: Arc<Mutex<Option<Fingerprint>>>,
}

impl FingerprintProbe {
    /// 新建一个采集器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 采集到的对端指纹。
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl PeerVerifier for FingerprintProbe {
    fn verify(&self, fp: &Fingerprint) -> bool {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(*fp);
        // 绝不因为「采集指纹」而信任一个还没被用户确认的设备。
        false
    }
}

/// 采集对端证书指纹(§3.3)。
///
/// 握手失败是**预期结果**:我们只要对端证书里的完整指纹。
pub async fn probe_fingerprint(request: ProbeRequest) -> Result<Fingerprint, BeamError> {
    let probe = FingerprintProbe::new();
    let net_config = net_config(&request.resolved, &request.identity);
    let verifier: Arc<dyn PeerVerifier> = Arc::new(probe.clone());
    // 故意让握手失败(校验器拒绝)。
    let _ = NetEndpoint::connect(request.addr, net_config, verifier).await;
    probe.fingerprint().ok_or_else(|| {
        BeamError::retryable(
            crate::error::E_CONNECTION_TIMEOUT,
            "未能采集到主机证书指纹(主机可能没有在运行 QUIC 服务)",
        )
    })
}

/// 起一个 QUIC 监听(§3.2),供桌面端「作为主机」使用。
///
/// 证书校验交给 `verifier`:主机侧传 [`beam_host::HostTrustGate`]
/// (产品方案 §8:未信任的对端**一律拒绝握手**,等用户人工确认后才写信任库)。
///
/// 监听本身**不**产生信任:能通过校验的对端一定已经在信任库里。
pub async fn listen_peer(
    request: ListenRequest,
    verifier: Arc<dyn PeerVerifier>,
) -> Result<beam_host::HostListener, BeamError> {
    let mut net_config = net_config(&request.resolved, &request.identity);
    // 监听用的是 `listen_addr`(`bind_addr` 是客户端侧的本地绑定地址)。
    net_config.listen_addr = request.bind_addr;
    beam_host::HostListener::listen(net_config, verifier)
        .await
        .map_err(|err| {
            BeamError::retryable(
                crate::error::E_CONNECTION_TIMEOUT,
                format!("QUIC 监听失败: {err}"),
            )
        })
}
