//! tracing 初始化（§3.15）。
//!
//! - JSON 行格式输出到轮转文件（可选同时输出到 stdout）；
//! - 热路径只允许 `trace!`，且默认级别不包含 `trace`；
//! - 字段规范由 [`crate::fields::LogFields`] 提供。

use std::path::PathBuf;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::error::LogError;
use crate::rolling::RollingFile;

/// 日志配置。
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// 日志目录；`None` 表示只输出到 stdout。
    pub dir: Option<PathBuf>,
    /// 日志文件名前缀。
    pub prefix: String,
    /// 最低日志级别；默认 `INFO`（`trace` 默认关闭，§3.15）。
    pub level: LevelFilter,
    /// 是否同时输出到 stdout。
    pub to_stdout: bool,
    /// 单文件上限（字节）。
    pub max_bytes: u64,
    /// 保留天数。
    pub max_days: u64,
    /// 保留文件数。
    pub max_files: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            dir: None,
            prefix: "beam".to_string(),
            level: LevelFilter::INFO,
            to_stdout: true,
            max_bytes: crate::rolling::DEFAULT_MAX_BYTES,
            max_days: crate::rolling::DEFAULT_MAX_DAYS,
            max_files: crate::rolling::DEFAULT_MAX_FILES,
        }
    }
}

impl LogConfig {
    /// 只输出到 stdout。
    pub fn stdout() -> Self {
        Self::default()
    }

    /// 输出到指定目录。
    pub fn with_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// 设置最低级别。
    pub fn with_level(mut self, level: LevelFilter) -> Self {
        self.level = level;
        self
    }
}

/// 已初始化的日志器。
///
/// 持有一个 `Option`，避免重复初始化时 panic。
#[derive(Debug, Default)]
pub struct Logger {
    _private: (),
}

/// 初始化全局日志（§3.15）。
///
/// 重复调用是安全的：第二次调用直接返回，不会 panic。
pub fn init(cfg: LogConfig) -> Result<Logger, LogError> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(cfg.level.to_string()));

    match cfg.dir.clone() {
        Some(dir) => {
            let rolling = RollingFile::new(&dir, &cfg.prefix)
                .map_err(|e| LogError::Init {
                    reason: format!("创建日志文件失败: {e}"),
                })?
                .with_max_bytes(cfg.max_bytes)
                .with_max_days(cfg.max_days)
                .with_max_files(cfg.max_files);
            let file_layer = fmt::layer().json().with_ansi(false).with_writer(rolling);
            let registry = tracing_subscriber::registry().with(filter).with(file_layer);
            if cfg.to_stdout {
                let stdout_layer = fmt::layer().with_ansi(false);
                let _ = registry.with(stdout_layer).try_init();
            } else {
                let _ = registry.try_init();
            }
        }
        None => {
            let stdout_layer = fmt::layer().with_ansi(false);
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .try_init();
        }
    }
    Ok(Logger { _private: () })
}

/// Windows 默认日志目录：`%APPDATA%/Beam/logs`。
pub fn default_log_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Beam").join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        let cfg = LogConfig::stdout();
        init(cfg.clone()).expect("首次应当成功");
        init(cfg).expect("重复初始化不应 panic");
    }

    #[test]
    fn file_logging_creates_directory() {
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let log_dir = dir.path().join("logs");
        let cfg = LogConfig {
            dir: Some(log_dir.clone()),
            to_stdout: false,
            ..LogConfig::default()
        };
        init(cfg).expect("应当能初始化");
        assert!(log_dir.exists());
    }
}
