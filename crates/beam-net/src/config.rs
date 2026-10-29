//! 网络配置（§3.2 配置项、§5 FEC 规则）。
//!
//! §3.2：`fec_ratio`（默认 0.2，码率 >150 Mbps 时下调到 0.1）、
//! `max_datagram=1200`、`jitter_frames`（有线 0，Wi-Fi 自适应 0–1）、`bitrate_min/max`。
//! §5：默认 ratio 0.2，码率 >150 Mbps 降到 0.1，丢包 >2% 升到 0.3。

use std::net::SocketAddr;
use std::time::Duration;

use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// §5 的码率阈值：超过 150 Mbps 时 FEC 比例下调。
pub const HIGH_BITRATE_BPS: u32 = 150_000_000;

/// §5 的丢包阈值：超过 2% 时 FEC 比例上调。
pub const HIGH_LOSS_PPM: u32 = 200;

/// 默认 FEC 比例。
pub const DEFAULT_FEC_RATIO: f32 = 0.2;

/// 默认最大数据报长度（§3.2 `max_datagram=1200`）。
pub const DEFAULT_MAX_DATAGRAM: usize = 1200;

/// 高码率时的 FEC 比例。
pub const HIGH_BITRATE_FEC_RATIO: f32 = 0.1;

/// 高丢包时的 FEC 比例。
pub const HIGH_LOSS_FEC_RATIO: f32 = 0.3;

/// 监听端所需的 TLS 身份（由 beam-pair 的 `Identity` 生成）。
#[derive(Debug)]
pub struct TlsIdentity {
    /// 证书链（自签，单证书）。
    pub cert_chain: Vec<CertificateDer<'static>>,
    /// 私钥。
    pub key: PrivateKeyDer<'static>,
}

impl Clone for TlsIdentity {
    fn clone(&self) -> Self {
        Self {
            cert_chain: self.cert_chain.clone(),
            // `PrivateKeyDer` 故意不实现 `Clone`（避免私钥被意外复制），
            // 这里显式调用 `clone_key` 生成独立副本。
            key: self.key.clone_key(),
        }
    }
}

/// 网络配置。
#[derive(Debug, Clone)]
pub struct NetConfig {
    /// FEC 冗余比例（§5）。
    pub fec_ratio: f32,
    /// 单个数据报最大长度（含 16 字节帧头，§3.2 `max_datagram=1200`）。
    pub max_datagram: usize,
    /// 抖动缓冲帧数（有线 0，Wi-Fi 自适应 0–1）。
    pub jitter_frames: u32,
    /// 码率下限。
    pub bitrate_min: u32,
    /// 码率上限。
    pub bitrate_max: u32,
    /// 连接超时。
    pub connect_timeout: Duration,
    /// 空闲超时。
    pub max_idle_timeout: Duration,
    /// TLS 服务器名（自签证书 SAN）。
    pub server_name: String,
    /// 监听地址（`listen` 使用）。
    pub listen_addr: SocketAddr,
    /// 客户端绑定地址（`connect` 使用，默认为随机端口）。
    pub bind_addr: SocketAddr,
    /// 监听端 TLS 身份；`None` 时 `listen` / `connect` 返回 `E2007`。
    pub identity: Option<TlsIdentity>,
}

impl Default for NetConfig {
    /// §3.2 的默认值。
    fn default() -> Self {
        Self {
            fec_ratio: DEFAULT_FEC_RATIO,
            max_datagram: DEFAULT_MAX_DATAGRAM,
            jitter_frames: 0,
            bitrate_min: 2_000_000,
            bitrate_max: 150_000_000,
            connect_timeout: Duration::from_secs(5),
            max_idle_timeout: Duration::from_secs(30),
            server_name: "beam.local".to_string(),
            listen_addr: SocketAddr::from(([0, 0, 0, 0], 4827)),
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            identity: None,
        }
    }
}

impl NetConfig {
    /// 单个数据报的净负载上限（扣掉 16 字节帧头，§5 `payload ≤1184 B`）。
    pub fn max_payload(&self) -> usize {
        self.max_datagram
            .saturating_sub(beam_proto::FRAME_HEADER_LEN)
    }
}

/// §5 FEC 比例自适应规则。
///
/// - 码率 > 150 Mbps → 0.1；
/// - 丢包 > 2% → 0.3；
/// - 其余 → `base`。
pub fn adaptive_fec_ratio(base: f32, bitrate_bps: u32, loss_ppm: u32) -> f32 {
    if loss_ppm > HIGH_LOSS_PPM {
        HIGH_LOSS_FEC_RATIO
    } else if bitrate_bps > HIGH_BITRATE_BPS {
        HIGH_BITRATE_FEC_RATIO
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_doc() {
        let cfg = NetConfig::default();
        assert_eq!(cfg.fec_ratio, 0.2);
        assert_eq!(cfg.max_datagram, 1200);
        assert_eq!(cfg.jitter_frames, 0);
        assert!(cfg.bitrate_min > 0);
        assert!(cfg.bitrate_max >= cfg.bitrate_min);
        assert_eq!(cfg.max_payload(), 1184);
    }

    #[test]
    fn fec_ratio_adapts_per_doc_rules() {
        assert_eq!(adaptive_fec_ratio(0.2, 40_000_000, 10), 0.2);
        assert_eq!(adaptive_fec_ratio(0.2, 200_000_000, 10), 0.1);
        assert_eq!(adaptive_fec_ratio(0.2, 40_000_000, 500), 0.3);
        // 高码率 + 高丢包时，丢包优先。
        assert_eq!(adaptive_fec_ratio(0.2, 200_000_000, 500), 0.3);
    }
}
