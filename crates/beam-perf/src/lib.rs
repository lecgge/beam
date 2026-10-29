//! # beam-perf —— Beam 性能统计（仅 Debug）
//!
//! 对应技术设计文档 **§3.13**（模块设计）与 **§7**（构建与发布）。
//!
//! ## 职责
//!
//! 各阶段耗时直方图（抓屏 / 颜色转换 / 编码 / 分包 / RTT / 丢包 /
//! FEC 恢复 / 抖动缓冲 / 解码 / 渲染 / 端到端）、码率、帧率、掉帧、
//! CPU/GPU/内存；10 分钟环形缓冲；CSV / JSON 导出。
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
//! - [`Perf::sample_system_usage`]：采样一次系统资源占用（CPU / GPU / 内存）。
//!
//! ## 子模块
//!
//! - [`stage`]：采集阶段枚举（含端到端 `Stage::E2E`）。
//! - [`histogram`]：定长桶耗时直方图。
//! - [`ring`]：10 分钟环形缓冲。
//! - [`clock`]：NTP 式四时间戳时钟同步与偏移估计（§3.13，每 5 s 一次）。
//! - [`e2e`]：回环上屏时间戳交叉校验（§3.13）。
//! - [`sysinfo`]：CPU / GPU / 内存的承载结构与可选采样入口。
//! - `collect`：**仅 `perf-stats` 开启时存在**的全局采集器实现。
//!
//! ## 性能
//!
//! 采集开销 < 1% CPU，单点 < 200 ns。实测数字见 `benches/record_overhead.rs`
//! （`cargo bench -p beam-perf`）。
//!
//! ## 测试
//!
//! 直方图分位数、环形缓冲淘汰、导出格式、feature 关闭时的空实现、
//! 时钟同步偏移估计、E2E 交叉校验。

#![forbid(unsafe_code)]

pub mod clock;
pub mod e2e;
pub mod histogram;
pub mod ring;
pub mod stage;
pub mod sysinfo;

#[cfg(feature = "perf-stats")]
pub mod collect;

pub use clock::{
    ClockSyncEstimate, ClockSyncSample, ClockSyncTracker, DEFAULT_SYNC_INTERVAL,
    DEFAULT_SYNC_INTERVAL_US,
};
pub use e2e::{cross_check, E2ECrossCheck, FramePipelineDurations, OpticalMeasurement};
pub use histogram::{Histogram, BUCKET_EDGES_NS};
pub use ring::{RingBuffer, Sample, DEFAULT_CAPACITY, DEFAULT_WINDOW_SECS};
pub use stage::{Stage, ALL_STAGES};
pub use sysinfo::{NoSystemUsageSource, SystemUsage, SystemUsageSource};

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
///
/// CPU / GPU / 内存三项以可选值承载：没有真实采样源时相关字段为 `None`，
/// **不会**用估算值顶替（见 [`Perf::sample_system_usage`]）。
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
    /// 最近一次系统资源占用采样。
    ///
    /// `None` 表示尚未采样，或采样源不可用。
    pub system: Option<SystemUsage>,
}

impl PerfSnapshot {
    /// CPU 占用百分比；没有采样数据时为 `None`。
    pub fn cpu_percent(&self) -> Option<f64> {
        self.system.as_ref().and_then(|u| u.cpu_percent)
    }

    /// GPU 占用百分比；没有采样数据时为 `None`。
    pub fn gpu_percent(&self) -> Option<f64> {
        self.system.as_ref().and_then(|u| u.gpu_percent)
    }

    /// 已用内存字节数；没有采样数据时为 `None`。
    pub fn memory_used_bytes(&self) -> Option<u64> {
        self.system.as_ref().and_then(|u| u.memory_used_bytes)
    }

    /// 内存总量字节数；没有采样数据时为 `None`。
    pub fn memory_total_bytes(&self) -> Option<u64> {
        self.system.as_ref().and_then(|u| u.memory_total_bytes)
    }
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

    /// 采样一次系统资源占用（CPU / GPU / 内存，§3.13）。
    ///
    /// **当前实现只提供承载与可选采样入口，未接真实系统采样源。**
    /// 未注册采样源（或采样源报告不可用）时返回 `None`，**不返回假数据**。
    ///
    /// 真实采样源由接入方实现 [`SystemUsageSource`] 并注册；
    /// 采样结果会进入后续的 [`Perf::snapshot`]。
    pub fn sample_system_usage() -> Option<SystemUsage> {
        #[cfg(feature = "perf-stats")]
        {
            collect::sample_system_usage()
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            None
        }
    }

    /// 注册系统资源占用采样源。
    ///
    /// feature 关闭时为空实现（采样入口恒返回 `None`）。
    pub fn set_system_usage_source(source: std::sync::Arc<dyn SystemUsageSource>) {
        #[cfg(feature = "perf-stats")]
        {
            collect::set_system_usage_source(source);
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            let _ = source;
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

    /// §3.13 要求：`perf_span!` 在 feature 关闭时必须展开为空。
    /// 用类型标注断言展开结果确实是 `()`，防止宏被误改成有开销的实现。
    #[cfg(not(feature = "perf-stats"))]
    #[test]
    fn perf_span_expands_to_unit_when_feature_disabled() {
        let _: () = crate::perf_span!("encode");
        let _: () = crate::perf_span!("e2e");
    }

    /// feature 开启时展开为作用域计时守卫。
    #[cfg(feature = "perf-stats")]
    #[test]
    fn perf_span_expands_to_span_when_feature_enabled() {
        let _: crate::Span = crate::perf_span!("encode");
    }

    #[test]
    fn perf_span_e2e_macro_compiles() {
        let guard = crate::perf_span!("e2e");
        let _ = guard;
    }

    #[test]
    fn system_metrics_are_optional_and_never_faked() {
        // 没有真实采样源时，采样入口返回 None，快照里的 system 也为 None。
        #[cfg(not(feature = "perf-stats"))]
        {
            assert_eq!(Perf::sample_system_usage(), None);
            assert_eq!(Perf::snapshot().system, None);
        }
        #[cfg(feature = "perf-stats")]
        {
            // feature 开启但未注册采样源时，同样拿不到数据。
            let _ = Perf::sample_system_usage();
        }
    }

    #[test]
    fn snapshot_has_system_slot_even_when_empty() {
        let s = PerfSnapshot::default();
        assert_eq!(s.system, None);
        assert_eq!(s.cpu_percent(), None);
        assert_eq!(s.gpu_percent(), None);
        assert_eq!(s.memory_used_bytes(), None);
    }
}
