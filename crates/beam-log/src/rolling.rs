//! 日志文件轮转（§3.15：JSON 文件轮转，7 天 / 50 MB）。
//!
//! 自己实现而不引入 `tracing-appender`，是为了让轮转策略完全可控：
//! - 单文件超过 `max_bytes`（默认 50 MB）立即切分；
//! - 文件超过 `max_days`（默认 7 天）在写入前清理；
//! - 文件名形如 `beam.log`、`beam.log.1`、`beam.log.2`……

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// 默认单文件上限（§3.15：50 MB）。
pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// 默认保留天数（§3.15：7 天）。
pub const DEFAULT_MAX_DAYS: u64 = 7;

/// 默认保留文件数。
pub const DEFAULT_MAX_FILES: usize = 16;

#[derive(Debug)]
struct State {
    file: Option<File>,
    path: PathBuf,
    written: u64,
    created: SystemTime,
}

/// 按大小与天数轮转的日志文件写入器。
#[derive(Debug)]
pub struct RollingFile {
    dir: PathBuf,
    prefix: String,
    max_bytes: u64,
    max_days: u64,
    max_files: usize,
    state: Mutex<State>,
}

impl RollingFile {
    /// 在 `dir` 下创建轮转写入器。
    pub fn new(dir: impl AsRef<Path>, prefix: impl Into<String>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            prefix: prefix.into(),
            max_bytes: DEFAULT_MAX_BYTES,
            max_days: DEFAULT_MAX_DAYS,
            max_files: DEFAULT_MAX_FILES,
            state: Mutex::new(State {
                file: None,
                path: PathBuf::new(),
                written: 0,
                created: SystemTime::now(),
            }),
        })
    }

    /// 设置单文件上限。
    pub fn with_max_bytes(mut self, v: u64) -> Self {
        self.max_bytes = v;
        self
    }

    /// 设置保留天数。
    pub fn with_max_days(mut self, v: u64) -> Self {
        self.max_days = v;
        self
    }

    /// 设置保留文件数。
    pub fn with_max_files(mut self, v: usize) -> Self {
        self.max_files = v;
        self
    }

    /// 当前日志文件路径。
    pub fn current_path(&self) -> PathBuf {
        self.state
            .lock()
            .map(|s| s.path.clone())
            .unwrap_or_else(|_| self.dir.join(format!("{}.log", self.prefix)))
    }

    fn open_locked(&self, state: &mut State) -> io::Result<()> {
        let path = self.dir.join(format!("{}.log", self.prefix));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        state.path = path;
        state.written = file.metadata().map(|m| m.len()).unwrap_or(0);
        state.created = SystemTime::now();
        state.file = Some(file);
        Ok(())
    }

    /// 切分：把 `beam.log` 顺延为 `beam.log.1`，`beam.log.1` 顺延为 `beam.log.2`……
    fn rotate_locked(&self, state: &mut State) -> io::Result<()> {
        state.file = None;
        for i in (1..self.max_files).rev() {
            let from = self.dir.join(format!("{}.log.{}", self.prefix, i));
            let to = self.dir.join(format!("{}.log.{}", self.prefix, i + 1));
            if from.exists() {
                let _ = std::fs::remove_file(&to);
                std::fs::rename(&from, &to)?;
            }
        }
        let base = self.dir.join(format!("{}.log", self.prefix));
        if base.exists() {
            let first = self.dir.join(format!("{}.log.1", self.prefix));
            let _ = std::fs::remove_file(&first);
            std::fs::rename(&base, &first)?;
        }
        self.open_locked(state)
    }

    /// 清理超过保留天数的旧文件。
    pub fn cleanup_expired(&self) -> io::Result<usize> {
        let max_age = Duration::from_secs(self.max_days.saturating_mul(24 * 3600));
        let now = SystemTime::now();
        let mut removed = 0usize;
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with(&self.prefix) {
                    continue;
                }
                let Ok(meta) = e.metadata() else { continue };
                let Ok(modified) = meta.modified() else {
                    continue;
                };
                if now.duration_since(modified).unwrap_or_default() > max_age {
                    let _ = std::fs::remove_file(e.path());
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("日志锁已中毒"))?;
        if state.file.is_none() {
            self.open_locked(&mut state)?;
        }
        if state.written > 0 && state.written + buf.len() as u64 > self.max_bytes {
            self.rotate_locked(&mut state)?;
        }
        if let Some(file) = state.file.as_mut() {
            file.write_all(buf)?;
            state.written += buf.len() as u64;
        }
        Ok(())
    }

    fn flush_inner(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("日志锁已中毒"))?;
        if let Some(file) = state.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

impl Write for RollingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_inner()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RollingFile {
    type Writer = RollingFileWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RollingFileWriter { file: self }
    }
}

/// [`RollingFile`] 的 `MakeWriter` 包装。
#[derive(Debug)]
pub struct RollingFileWriter<'a> {
    file: &'a RollingFile,
}

impl Write for RollingFileWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn writes_go_to_log_file() {
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let mut f = RollingFile::new(dir.path(), "beam").expect("应当能创建");
        f.write_all(b"hello\n").expect("应当能写");
        f.flush().expect("应当能刷");
        let content = std::fs::read_to_string(dir.path().join("beam.log")).expect("应当能读");
        assert!(content.contains("hello"));
    }

    #[test]
    fn rotates_when_size_limit_exceeded() {
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let mut f = RollingFile::new(dir.path(), "beam")
            .expect("应当能创建")
            .with_max_bytes(16);
        for _ in 0..4 {
            f.write_all(b"0123456789abcdef").expect("应当能写");
        }
        f.flush().expect("应当能刷");
        assert!(dir.path().join("beam.log.1").exists(), "应当产生轮转文件");
    }

    #[test]
    fn cleanup_removes_old_files() {
        let dir = tempfile::tempdir().expect("应当能建临时目录");
        let f = RollingFile::new(dir.path(), "beam")
            .expect("应当能创建")
            .with_max_days(0);
        std::fs::write(dir.path().join("beam.log.9"), b"old").expect("应当能写");
        let removed = f.cleanup_expired().expect("应当能清理");
        assert!(removed >= 1);
    }
}
