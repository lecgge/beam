//! # beam-perf —— Beam 性能统计（仅 Debug）
//!
//! 对应技术设计文档 **§3.13**（模块设计）与 **§7**（构建与发布）。
//!
//! ## 职责
//!
//! 各阶段耗时直方图（抓屏 / 颜色转换 / 编码 / 分包 / RTT / 丢包 /
//! FEC 恢复 / 抖动缓冲 / 解码 / 渲染 / 端到端）、码率、帧率、掉帧；
//! 10 分钟环形缓冲；CSV / JSON 导出。
//!
//! ## 门控（§3.13 / §7）
//!
//! - cargo feature `perf-stats`，**默认关闭**，release 禁止开启（CI 用
//!   `cargo tree -e features` 断言它未进入 release 解析结果）；
//! - 采集宏 `perf_span!("encode")` 在 feature 关闭时展开为空；
//! - Android 侧只允许 `debugImplementation` 依赖 `:feature-perf`。
//!
//! ## 接口
//!
//! - [`Perf::record`]：记录一次耗时。
//! - [`Perf::snapshot`]：取当前快照。
//! - [`Perf::export`]：导出 CSV / JSON。
//! - [`perf_span!`]：作用域计时宏。
//!
//! ## 性能
//!
//! 采集开销 < 1% CPU，单点 < 200 ns。
//!
//! ## 测试
//!
//! 直方图分位数、环形缓冲淘汰、导出格式、feature 关闭时的空实现。

#![forbid(unsafe_code)]

pub mod histogram;
pub mod ring;
pub mod stage;

#[cfg(feature = "perf-stats")]
pub mod collect;

pub use histogram::{Histogram, BUCKET_EDGES_NS};
pub use ring::{RingBuffer, Sample, DEFAULT_CAPACITY, DEFAULT_WINDOW_SECS};
pub use stage::{Stage, ALL_STAGES};

use std::time::Duration;

/// 导出格式（§3.13）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// CSV。
    Csv,
    /// JSON。
    Json,
}

impl ExportFormat {
    /// 由名字解析。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "csv" => Some(ExportFormat::Csv),
            "json" => Some(ExportFormat::Json),
            _ => None,
        }
    }
}

/// 性能层错误（§3.13 使用 `E13xxx` 段）。
#[derive(Debug, thiserror::Error)]
pub enum PerfError {
    /// `E13001`：`perf-stats` feature 未开启。
    #[error("E13001: 性能统计未开启(缺少 perf-stats feature)")]
    Disabled,

    /// `E13002`：导出失败。
    #[error("E13002: 导出失败: {reason}")]
    ExportFailed {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl PerfError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            PerfError::Disabled => 13001,
            PerfError::ExportFailed { .. } => 13002,
        }
    }

    /// 返回 `E` 前缀错误码字符串。
    pub fn code_str(&self) -> String {
        format!("E{}", self.code())
    }
}

/// 单阶段快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageSnapshot {
    /// 样本数。
    pub count: u64,
    /// 平均耗时，纳秒。
    pub mean_ns: u64,
    /// p50，纳秒。
    pub p50_ns: u64,
    /// p95，纳秒。
    pub p95_ns: u64,
    /// p99，纳秒。
    pub p99_ns: u64,
    /// 最大耗时，纳秒。
    pub max_ns: u64,
}

/// 性能快照（§3.13 `PerfSnapshot`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerfSnapshot {
    /// 各阶段耗时统计。
    pub stages: Vec<(Stage, StageSnapshot)>,
    /// 采样点数。
    pub samples: usize,
    /// 当前码率，bit/s。
    pub bitrate_bps: u32,
    /// 当前帧率。
    pub fps: u32,
    /// 累计掉帧数。
    pub dropped_frames: u32,
}

/// 性能统计入口（§3.13 `Perf::record/snapshot/export`）。
///
/// feature 关闭时全部为空实现，保证调用方代码在任何构建下都能编译。
pub struct Perf;

impl Perf {
    /// 记录一次阶段耗时。
    pub fn record(_stage: Stage, _dur: Duration) {
        #[cfg(feature = "perf-stats")]
        collect::record(_stage, _dur);
    }

    /// 取当前快照。
    pub fn snapshot() -> PerfSnapshot {
        #[cfg(feature = "perf-stats")]
        {
            collect::snapshot()
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            PerfSnapshot::default()
        }
    }

    /// 导出到文件（CSV / JSON）。
    pub fn export(fmt: ExportFormat, path: &std::path::Path) -> Result<(), PerfError> {
        #[cfg(feature = "perf-stats")]
        {
            collect::export(fmt, path)
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            let _ = (fmt, path);
            Err(PerfError::Disabled)
        }
    }

    /// 记录当前码率 / 帧率 / 掉帧。
    pub fn set_counters(bitrate_bps: u32, fps: u32, dropped_frames: u32) {
        #[cfg(feature = "perf-stats")]
        {
            collect::set_counters(bitrate_bps, fps, dropped_frames);
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            let _ = (bitrate_bps, fps, dropped_frames);
        }
    }

    /// 清空所有统计。
    pub fn reset() {
        #[cfg(feature = "perf-stats")]
        {
            collect::reset();
        }
    }
}

/// 作用域计时守卫（仅在 `perf-stats` 开启时存在）。
#[cfg(feature = "perf-stats")]
#[derive(Debug)]
pub struct Span {
    stage: Stage,
    start: std::time::Instant,
}

#[cfg(feature = "perf-stats")]
impl Span {
    /// 开始计时。
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            start: std::time::Instant::now(),
        }
    }
}

#[cfg(feature = "perf-stats")]
impl Drop for Span {
    fn drop(&mut self) {
        Perf::record(self.stage, self.start.elapsed());
    }
}

/// 作用域计时宏：`perf_span!("encode")`。
///
/// `perf-stats` 关闭时展开为空（`()`），调用方无需条件编译。
#[cfg(feature = "perf-stats")]
#[macro_export]
macro_rules! perf_span {
    ($stage:literal) => {{
        let __beam_perf_span = $crate::Span::new($crate::Stage::from_name($stage));
        __beam_perf_span
    }};
}

/// 作用域计时宏：`perf_span!("encode")`（feature 关闭时为空实现）。
#[cfg(not(feature = "perf-stats"))]
#[macro_export]
macro_rules! perf_span {
    ($stage:literal) => {{
        ()
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_format_parsing() {
        assert_eq!(ExportFormat::from_name("csv"), Some(ExportFormat::Csv));
        assert_eq!(ExportFormat::from_name("JSON"), Some(ExportFormat::Json));
        assert_eq!(ExportFormat::from_name("xml"), None);
    }

    #[test]
    fn snapshot_without_feature_is_empty() {
        let s = Perf::snapshot();
        #[cfg(not(feature = "perf-stats"))]
        {
            assert!(s.stages.is_empty());
            assert_eq!(s.samples, 0);
        }
        #[cfg(feature = "perf-stats")]
        {
            let _ = s;
        }
    }

    #[test]
    fn perf_span_macro_compiles_in_both_modes() {
        let guard = crate::perf_span!("encode");
        let _ = guard;
    }
}
