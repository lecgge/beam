//! 二维码配对载荷（§3.3 `beam://pair?h=ip:port&fp=..&t=token`）。
//!
//! §产品方案 §8：载荷在局域网上是明文，旁观者扫到即获得配对资格，
//! 因此**主机端必须弹窗人工确认后才会写入信任库**——该确认不可省略。

use crate::error::PairError;
use crate::fingerprint::Fingerprint;

/// URI 前缀。
pub const URI_PREFIX: &str = "beam://pair?";

/// 二维码配对载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairPayload {
    /// 主机地址（IP）。
    pub host: String,
    /// 主机端口。
    pub port: u16,
    /// 主机证书指纹。
    pub fingerprint: Fingerprint,
    /// 一次性 token（§3.3 `pin_ttl=120s`）。
    pub token: String,
}

impl PairPayload {
    /// 序列化为二维码文本。
    pub fn to_uri(&self) -> String {
        format!(
            "{URI_PREFIX}h={}:{}&fp={}&t={}",
            self.host,
            self.port,
            self.fingerprint.to_hex(),
            self.token
        )
    }

    /// 解析二维码文本。
    pub fn from_uri(uri: &str) -> Result<Self, PairError> {
        let rest = uri
            .strip_prefix(URI_PREFIX)
            .ok_or_else(|| PairError::InvalidPayload {
                reason: format!("缺少前缀 {URI_PREFIX}"),
            })?;

        let mut host = None;
        let mut fingerprint = None;
        let mut token = None;

        for part in rest.split('&') {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| PairError::InvalidPayload {
                    reason: format!("查询参数缺少 '=': {part}"),
                })?;
            match key {
                "h" => host = Some(value.to_string()),
                "fp" => fingerprint = Some(Fingerprint::from_hex(value)?),
                "t" => token = Some(value.to_string()),
                _ => {}
            }
        }

        let host = host.ok_or_else(|| PairError::InvalidPayload {
            reason: "缺少主机地址 h".to_string(),
        })?;

        // 端口写在 h 里，形如 `192.168.1.10:4827`。
        let (ip, port_str) = host
            .rsplit_once(':')
            .ok_or_else(|| PairError::InvalidPayload {
                reason: format!("主机地址缺少端口: {host}"),
            })?;
        let port: u16 = port_str.parse().map_err(|_| PairError::InvalidPayload {
            reason: format!("端口非法: {port_str}"),
        })?;

        Ok(Self {
            host: ip.to_string(),
            port,
            fingerprint: fingerprint.ok_or_else(|| PairError::InvalidPayload {
                reason: "缺少指纹 fp".to_string(),
            })?,
            token: token.ok_or_else(|| PairError::InvalidPayload {
                reason: "缺少 token t".to_string(),
            })?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PairPayload {
        PairPayload {
            host: "192.168.1.10".to_string(),
            port: 4827,
            fingerprint: Fingerprint::from_bytes([0x11; 32]),
            token: "abcdef0123456789".to_string(),
        }
    }

    #[test]
    fn uri_round_trip() {
        let p = sample();
        let uri = p.to_uri();
        assert!(uri.starts_with(URI_PREFIX));
        let back = PairPayload::from_uri(&uri).expect("应当能解回");
        assert_eq!(back, p);
    }

    #[test]
    fn uri_has_expected_shape() {
        let uri = sample().to_uri();
        assert!(uri.contains("h=192.168.1.10:4827"));
        assert!(uri.contains("fp="));
        assert!(uri.contains("t=abcdef0123456789"));
    }

    #[test]
    fn wrong_prefix_is_rejected() {
        let err = PairPayload::from_uri("http://example.com").expect_err("前缀错误必须失败");
        assert_eq!(err.code(), 3007);
    }
}
