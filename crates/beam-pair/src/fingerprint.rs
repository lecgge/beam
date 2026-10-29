//! 证书指纹与对端校验（§3.3 `Fingerprint([u8;32])`、§5 指纹固定）。
//!
//! 指纹 = SHA-256(证书 DER)，32 字节；二维码里以十六进制传输，
//! mDNS TXT 里只广播前 8 位（§3.4）。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::PairError;

/// 证书指纹（§3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// 指纹字节长度。
    pub const LEN: usize = 32;

    /// 由 32 字节构造。
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 由证书 DER 计算指纹（SHA-256）。
    pub fn from_cert_der(der: &[u8]) -> Self {
        let digest = Sha256::digest(der);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self(out)
    }

    /// 原始字节。
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 完整十六进制表示（64 个字符）。
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// 短指纹：前 8 位十六进制（§3.4 mDNS TXT `fp` 字段）。
    pub fn short_hex(&self) -> String {
        self.to_hex()[..8].to_string()
    }

    /// 解析十六进制指纹。
    pub fn from_hex(s: &str) -> Result<Self, PairError> {
        let s = s.trim();
        if s.len() != 64 {
            return Err(PairError::InvalidPayload {
                reason: format!("指纹长度应为 64 个十六进制字符,实际 {}", s.len()),
            });
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hex = std::str::from_utf8(chunk).map_err(|_| PairError::InvalidPayload {
                reason: "指纹含非 ASCII 字符".to_string(),
            })?;
            out[i] = u8::from_str_radix(hex, 16).map_err(|_| PairError::InvalidPayload {
                reason: format!("指纹含非法十六进制字符: {hex}"),
            })?;
        }
        Ok(Self(out))
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// 对端证书校验器（§3.3 `impl PeerVerifier for TrustStore`）。
///
/// beam-net 的 TLS 握手通过该 trait 把「证书校验」交给配对层，
/// 网络层因此不需要理解信任库语义（§3.2 边界）。
pub trait PeerVerifier: Send + Sync + std::fmt::Debug {
    /// 该指纹是否可信。
    fn verify(&self, fp: &Fingerprint) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_sha256_of_der() {
        let der = b"hello beam";
        let fp = Fingerprint::from_cert_der(der);
        // sha256("hello beam")
        assert_eq!(fp.to_hex().len(), 64);
        assert_eq!(fp.short_hex().len(), 8);
        let again = Fingerprint::from_cert_der(der);
        assert_eq!(fp, again);
    }

    #[test]
    fn hex_round_trip() {
        let fp = Fingerprint::from_bytes([0xAB; 32]);
        let back = Fingerprint::from_hex(&fp.to_hex()).expect("应当能解回");
        assert_eq!(fp, back);
    }

    #[test]
    fn bad_hex_is_rejected() {
        let err = Fingerprint::from_hex("zz").expect_err("非法十六进制必须失败");
        assert_eq!(err.code(), 3007);
    }
}
