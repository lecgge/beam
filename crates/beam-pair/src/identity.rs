//! 本机身份：Ed25519 自签证书与私钥保管（§3.3 `Identity::load_or_create(store)`）。
//!
//! §3.3 要求「Ed25519 自签证书(rcgen)」。
//! §3.14 要求私钥在 Windows 上用 DPAPI 加密保存；
//! 本 crate 只定义 [`KeyStore`] 抽象，具体加密后端由上层（beam-config / beam-host）注入，
//! 以免配对层与存储层耦合。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rand::RngCore;
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use time::{Duration as TimeDuration, OffsetDateTime};

use crate::error::PairError;
use crate::fingerprint::Fingerprint;
use crate::qr::PairPayload;

/// 身份材料存储后端。
pub trait KeyStore: Send + Sync {
    /// 读取已保存的身份材料；没有则返回 `None`。
    fn load(&self) -> Result<Option<Vec<u8>>, PairError>;
    /// 保存身份材料。
    fn save(&self, blob: &[u8]) -> Result<(), PairError>;
}

/// 内存实现：进程重启即丢失，仅用于测试与临时会话。
#[derive(Debug, Default)]
pub struct MemoryKeyStore {
    inner: Mutex<Option<Vec<u8>>>,
}

impl MemoryKeyStore {
    /// 构造空的内存存储。
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStore for MemoryKeyStore {
    fn load(&self) -> Result<Option<Vec<u8>>, PairError> {
        Ok(self.inner.lock().map(|g| g.clone()).unwrap_or(None))
    }

    fn save(&self, blob: &[u8]) -> Result<(), PairError> {
        let mut guard = self.inner.lock().map_err(|_| PairError::Storage {
            reason: "内存锁已中毒".to_string(),
        })?;
        *guard = Some(blob.to_vec());
        Ok(())
    }
}

/// 明文文件实现。
///
/// **注意**：生产环境应使用 DPAPI 加密后端（§3.14），
/// 这里只保留最小可用实现，避免把平台加密细节写进配对层。
#[derive(Debug, Clone)]
pub struct FileKeyStore {
    path: PathBuf,
}

impl FileKeyStore {
    /// 指定文件路径。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl KeyStore for FileKeyStore {
    fn load(&self) -> Result<Option<Vec<u8>>, PairError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PairError::Storage {
                reason: format!("读取 {} 失败: {e}", self.path.display()),
            }),
        }
    }

    fn save(&self, blob: &[u8]) -> Result<(), PairError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| PairError::Storage {
                reason: format!("创建目录 {} 失败: {e}", dir.display()),
            })?;
        }
        std::fs::write(&self.path, blob).map_err(|e| PairError::Storage {
            reason: format!("写入 {} 失败: {e}", self.path.display()),
        })
    }
}

/// 身份材料 blob 魔数。
const BLOB_MAGIC: &[u8; 8] = b"BEAMID01";

/// 本机身份（证书 + 私钥 + 指纹）。
#[derive(Debug, Clone)]
pub struct Identity {
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
    fingerprint: Fingerprint,
}

impl Identity {
    /// 载入已有身份；没有则生成一个新的并写入 `store`。
    pub fn load_or_create(store: &dyn KeyStore) -> Result<Self, PairError> {
        if let Some(blob) = store.load()? {
            if let Some(id) = Self::from_blob(&blob) {
                return Ok(id);
            }
        }
        let id = Self::generate()?;
        store.save(&id.to_blob())?;
        Ok(id)
    }

    /// 生成一个新的 Ed25519 自签身份。
    pub fn generate() -> Result<Self, PairError> {
        let key_pair = KeyPair::generate_for(&PKCS_ED25519).map_err(|e| PairError::Storage {
            reason: format!("生成 Ed25519 密钥对失败: {e}"),
        })?;

        let mut params = CertificateParams::new(vec!["beam.local".to_string()]).map_err(|e| {
            PairError::Storage {
                reason: format!("构造证书参数失败: {e}"),
            }
        })?;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - TimeDuration::days(1);
        params.not_after = now + TimeDuration::days(3650);

        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| PairError::Storage {
                reason: format!("自签证书失败: {e}"),
            })?;

        let cert_der = cert.der().to_vec();
        let key_der = key_pair.serialize_der();
        let fingerprint = Fingerprint::from_cert_der(&cert_der);
        Ok(Self {
            cert_der,
            key_der,
            fingerprint,
        })
    }

    /// 本机指纹。
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    /// 证书链（自签，单证书）。
    pub fn certificate_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![CertificateDer::from(self.cert_der.clone())]
    }

    /// 私钥（PKCS#8）。
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()))
    }

    /// 生成二维码配对载荷（§3.3 `beam://pair?h=ip:port&fp=..&t=token`）。
    pub fn qr_payload(&self, addr: SocketAddr) -> PairPayload {
        PairPayload {
            host: addr.ip().to_string(),
            port: addr.port(),
            fingerprint: self.fingerprint,
            token: generate_token(),
        }
    }

    fn to_blob(&self) -> Vec<u8> {
        let mut blob =
            Vec::with_capacity(BLOB_MAGIC.len() + 4 + self.cert_der.len() + self.key_der.len());
        blob.extend_from_slice(BLOB_MAGIC);
        blob.extend_from_slice(&(self.cert_der.len() as u32).to_be_bytes());
        blob.extend_from_slice(&self.cert_der);
        blob.extend_from_slice(&self.key_der);
        blob
    }

    fn from_blob(blob: &[u8]) -> Option<Self> {
        if blob.len() < BLOB_MAGIC.len() + 4 || &blob[..BLOB_MAGIC.len()] != BLOB_MAGIC {
            return None;
        }
        let len = u32::from_be_bytes([blob[8], blob[9], blob[10], blob[11]]) as usize;
        let cert_end = 12usize.checked_add(len)?;
        if blob.len() < cert_end {
            return None;
        }
        let cert_der = blob[12..cert_end].to_vec();
        let key_der = blob[cert_end..].to_vec();
        if key_der.is_empty() {
            return None;
        }
        let fingerprint = Fingerprint::from_cert_der(&cert_der);
        Some(Self {
            cert_der,
            key_der,
            fingerprint,
        })
    }
}

/// 生成 128 位随机 token 的十六进制串（§3.3 `pin_ttl=120s` 一次性 token）。
pub fn generate_token() -> String {
    let mut buf = [0u8; 16];
    rand::rng().fill_bytes(&mut buf);
    let mut s = String::with_capacity(32);
    for b in buf {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 读取身份存储文件路径（Windows：`%APPDATA%/Beam/identity.key`）。
pub fn default_identity_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join("Beam").join("identity.key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_is_stable() {
        let store = MemoryKeyStore::new();
        let a = Identity::load_or_create(&store).expect("首次应当成功");
        let b = Identity::load_or_create(&store).expect("第二次应当复用");
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn different_identities_have_different_fingerprints() {
        let a = Identity::generate().expect("应当能生成");
        let b = Identity::generate().expect("应当能生成");
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn blob_round_trip() {
        let id = Identity::generate().expect("应当能生成");
        let back = Identity::from_blob(&id.to_blob()).expect("应当能解回");
        assert_eq!(id.fingerprint(), back.fingerprint());
        assert_eq!(id.certificate_chain().len(), 1);
    }

    #[test]
    fn corrupted_blob_is_rejected() {
        assert!(Identity::from_blob(b"garbage").is_none());
    }
}
