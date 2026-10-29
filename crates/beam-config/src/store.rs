//! 配置持久化（§3.14）。
//!
//! - Windows：`%APPDATA%/Beam/config.toml`（用户配置）、`%APPDATA%/Beam/trust.json`（信任库）；
//! - 私钥用 DPAPI 加密保存：**这里只留 [`SecretProtector`] 抽象**，
//!   平台实现由 beam-host 注入，避免配置层依赖 Windows API。

use std::path::{Path, PathBuf};

use crate::error::ConfigError;
use crate::model::{ConfigLayers, Overrides};

/// 配置存储后端。
pub trait ConfigStore: Send + Sync {
    /// 读取原始文本；不存在返回 `None`。
    fn load(&self) -> Result<Option<String>, ConfigError>;
    /// 写入原始文本。
    fn save(&self, content: &str) -> Result<(), ConfigError>;
}

/// TOML 文件存储。
#[derive(Debug, Clone)]
pub struct TomlFileStore {
    path: PathBuf,
}

impl TomlFileStore {
    /// 指定文件路径。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ConfigStore for TomlFileStore {
    fn load(&self) -> Result<Option<String>, ConfigError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::StorageUnavailable {
                reason: format!("读取 {} 失败: {e}", self.path.display()),
            }),
        }
    }

    fn save(&self, content: &str) -> Result<(), ConfigError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ConfigError::StorageUnavailable {
                reason: format!("创建目录 {} 失败: {e}", dir.display()),
            })?;
        }
        std::fs::write(&self.path, content).map_err(|e| ConfigError::StorageUnavailable {
            reason: format!("写入 {} 失败: {e}", self.path.display()),
        })
    }
}

/// 内存存储（测试用）。
#[derive(Debug, Default)]
pub struct MemoryStore {
    content: std::sync::Mutex<Option<String>>,
}

impl MemoryStore {
    /// 构造空存储。
    pub fn new() -> Self {
        Self::default()
    }
}

impl ConfigStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, ConfigError> {
        Ok(self.content.lock().map(|g| g.clone()).unwrap_or(None))
    }

    fn save(&self, content: &str) -> Result<(), ConfigError> {
        let mut guard = self
            .content
            .lock()
            .map_err(|_| ConfigError::StorageUnavailable {
                reason: "内存锁已中毒".to_string(),
            })?;
        *guard = Some(content.to_string());
        Ok(())
    }
}

/// 敏感数据保护器（Windows 上应为 DPAPI）。
pub trait SecretProtector: Send + Sync {
    /// 加密明文。
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, ConfigError>;
    /// 解密密文。
    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, ConfigError>;
}

/// 明文保护器：**仅用于开发与测试**。
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaintextProtector;

impl SecretProtector for PlaintextProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, ConfigError> {
        Ok(plain.to_vec())
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, ConfigError> {
        Ok(blob.to_vec())
    }
}

/// Windows 用户配置路径：`%APPDATA%/Beam/config.toml`。
pub fn default_config_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join("Beam").join("config.toml"))
}

/// Windows 信任库路径：`%APPDATA%/Beam/trust.json`。
pub fn default_trust_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join("Beam").join("trust.json"))
}

/// 解析 TOML 文本为配置层。
pub fn parse_layers(content: &str) -> Result<ConfigLayers, ConfigError> {
    let value: toml::Value = toml::from_str(content).map_err(|e| ConfigError::ParseFailed {
        reason: format!("TOML 解析失败: {e}"),
    })?;
    let overrides: Overrides =
        value
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError::ParseFailed {
                reason: format!("配置结构不匹配: {e}"),
            })?;
    Ok(ConfigLayers {
        file: overrides,
        ..Default::default()
    })
}

/// 把配置层序列化成 TOML 文本（只写 `file` 层）。
pub fn to_toml(layers: &ConfigLayers) -> Result<String, ConfigError> {
    toml::to_string_pretty(&layers.file).map_err(|e| ConfigError::StorageUnavailable {
        reason: format!("序列化配置失败: {e}"),
    })
}

/// 从存储读取配置文件层。
pub fn load_file_layer(store: &dyn ConfigStore) -> Result<ConfigLayers, ConfigError> {
    match store.load()? {
        Some(content) => parse_layers(&content),
        None => Ok(ConfigLayers::default()),
    }
}

/// 把配置层写入存储。
pub fn save_file_layer(store: &dyn ConfigStore, layers: &ConfigLayers) -> Result<(), ConfigError> {
    store.save(&to_toml(layers)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip() {
        let mut layers = ConfigLayers::default();
        layers.file.video.width = Some(2560);
        layers.file.network.fec_ratio = Some(0.3);
        let text = to_toml(&layers).expect("应当能序列化");
        let back = parse_layers(&text).expect("应当能解析");
        assert_eq!(back.file.video.width, Some(2560));
        assert_eq!(back.file.network.fec_ratio, Some(0.3));
    }

    #[test]
    fn broken_toml_is_parse_error() {
        let err = parse_layers("this is not toml =").expect_err("非法 TOML 必须失败");
        assert_eq!(err.code(), 11001);
    }

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::new();
        assert!(load_file_layer(&store)
            .expect("应当能读")
            .file
            .video
            .width
            .is_none());
        let mut layers = ConfigLayers::default();
        layers.file.video.height = Some(1440);
        save_file_layer(&store, &layers).expect("应当能写");
        let back = load_file_layer(&store).expect("应当能读");
        assert_eq!(back.file.video.height, Some(1440));
    }

    #[test]
    fn file_store_round_trip() {
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let store = TomlFileStore::new(dir.path().join("config.toml"));
        let mut layers = ConfigLayers::default();
        layers.file.video.fps = Some(144);
        save_file_layer(&store, &layers).expect("应当能写");
        let back = load_file_layer(&store).expect("应当能读");
        assert_eq!(back.file.video.fps, Some(144));
    }

    #[test]
    fn plaintext_protector_round_trip() {
        let p = PlaintextProtector;
        let blob = p.protect(b"secret").expect("应当能加密");
        assert_eq!(p.unprotect(&blob).expect("应当能解密"), b"secret");
    }
}
