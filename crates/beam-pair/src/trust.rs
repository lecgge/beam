//! 信任库（§3.3 `TrustedPeer{fp,name,platform,paired_at,perms}`、`TrustStore`）。
//!
//! §产品方案 §8：扫码链路在局域网上是明文，
//! 因此**主机端必须弹窗人工确认后才会写入信任库**，本模块只负责存取。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::PairError;
use crate::fingerprint::{Fingerprint, PeerVerifier};

/// 对端权限（§3.8 按设备做权限控制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerPerms {
    /// 允许观看画面。
    pub view: bool,
    /// 允许回传键鼠输入。
    pub input: bool,
    /// 允许传输音频。
    pub audio: bool,
    /// 允许使用手柄。
    pub gamepad: bool,
}

impl Default for PeerPerms {
    fn default() -> Self {
        Self {
            view: true,
            input: true,
            audio: true,
            gamepad: false,
        }
    }
}

/// 已配对设备（§3.3 `TrustedPeer`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeer {
    /// 证书指纹。
    pub fp: Fingerprint,
    /// 设备名。
    pub name: String,
    /// 平台（`windows` / `android`）。
    pub platform: String,
    /// 配对时间，Unix 秒。
    pub paired_at: u64,
    /// 权限。
    pub perms: PeerPerms,
}

impl TrustedPeer {
    /// 构造一个已配对设备。
    pub fn new(
        fp: Fingerprint,
        name: impl Into<String>,
        platform: impl Into<String>,
        paired_at: u64,
    ) -> Self {
        Self {
            fp,
            name: name.into(),
            platform: platform.into(),
            paired_at,
            perms: PeerPerms::default(),
        }
    }
}

/// 信任库（§3.14 `trust.json`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    /// 已配对设备列表。
    pub peers: Vec<TrustedPeer>,
}

impl TrustStore {
    /// 构造空信任库。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `trust.json` 载入；文件不存在返回空库。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PairError> {
        let path = path.as_ref();
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| PairError::Storage {
                reason: format!("解析 {} 失败: {e}", path.display()),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(PairError::Storage {
                reason: format!("读取 {} 失败: {e}", path.display()),
            }),
        }
    }

    /// 写入 `trust.json`。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), PairError> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| PairError::Storage {
                reason: format!("创建目录 {} 失败: {e}", dir.display()),
            })?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| PairError::Storage {
            reason: format!("序列化信任库失败: {e}"),
        })?;
        std::fs::write(path, bytes).map_err(|e| PairError::Storage {
            reason: format!("写入 {} 失败: {e}", path.display()),
        })
    }

    /// 加入或更新一个已配对设备（按指纹去重）。
    pub fn upsert(&mut self, peer: TrustedPeer) {
        match self.peers.iter_mut().find(|p| p.fp == peer.fp) {
            Some(existing) => *existing = peer,
            None => self.peers.push(peer),
        }
    }

    /// 移除设备，返回是否确实移除了。
    pub fn remove(&mut self, fp: &Fingerprint) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| &p.fp != fp);
        self.peers.len() != before
    }

    /// 查询设备。
    pub fn get(&self, fp: &Fingerprint) -> Option<&TrustedPeer> {
        self.peers.iter().find(|p| &p.fp == fp)
    }

    /// 全部已配对设备。
    pub fn peers(&self) -> &[TrustedPeer] {
        &self.peers
    }

    /// 指纹是否可信。
    pub fn is_trusted(&self, fp: &Fingerprint) -> bool {
        self.get(fp).is_some()
    }
}

impl PeerVerifier for TrustStore {
    fn verify(&self, fp: &Fingerprint) -> bool {
        self.is_trusted(fp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_is_idempotent_per_fingerprint() {
        let mut store = TrustStore::new();
        let fp = Fingerprint::from_bytes([3u8; 32]);
        store.upsert(TrustedPeer::new(fp, "手机", "android", 100));
        store.upsert(TrustedPeer::new(fp, "手机2", "android", 200));
        assert_eq!(store.peers().len(), 1);
        assert_eq!(store.get(&fp).expect("应当存在").name, "手机2");
    }

    #[test]
    fn verifier_accepts_only_known_fingerprints() {
        let mut store = TrustStore::new();
        let known = Fingerprint::from_bytes([4u8; 32]);
        store.upsert(TrustedPeer::new(known, "平板", "android", 1));
        assert!(store.verify(&known));
        assert!(!store.verify(&Fingerprint::from_bytes([5u8; 32])));
    }

    #[test]
    fn remove_works() {
        let mut store = TrustStore::new();
        let fp = Fingerprint::from_bytes([6u8; 32]);
        store.upsert(TrustedPeer::new(fp, "x", "windows", 1));
        assert!(store.remove(&fp));
        assert!(!store.remove(&fp));
        assert!(store.peers().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let path = dir.path().join("trust.json");
        let mut store = TrustStore::new();
        let fp = Fingerprint::from_bytes([7u8; 32]);
        store.upsert(TrustedPeer::new(fp, "电视", "android", 42));
        store.save(&path).expect("应当能保存");

        let loaded = TrustStore::load(&path).expect("应当能载入");
        assert_eq!(loaded, store);
    }
}
