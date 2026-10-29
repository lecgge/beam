//! 系统资源占用（CPU / GPU / 内存）的承载与可选采样入口（§3.13）。
//!
//! §3.13 要求性能快照包含「CPU/GPU/内存」。本模块提供三样东西：
//!
//! - [`SystemUsage`]：一次采样的承载结构。每个字段都是 `Option`，
//!   没有采样源时为 `None`，**不编造数字**；
//! - [`SystemUsageSource`]：采样源接口。真实采样（Windows 进程 CPU 时间、
//!   GPU 适配器负载、进程工作集）由接入方实现，再通过
//!   [`crate::Perf::set_system_usage_source`] 注册；
//! - [`crate::Perf::sample_system_usage`]：采样入口。
//!
//! ## 当前实现边界
//!
//! **当前实现只提供承载与可选采样入口，未接真实系统采样源。**
//!
//! - 未注册采样源时，采样入口恒返回 `None`；
//! - 未注册采样源时，快照里的 `system_usage` 恒为 `None`；
//! - 本 crate 不内置任何平台相关依赖（`windows` / `libc` 等），
//!   真实系统采样属于跨 crate 接入范围，由后续批次完成。
//!
//! 之所以坚持「拿不到就返回 `None`」，是因为编造出来的 CPU/GPU/内存数字
//! 比没有数字更糟：它会让性能问题被错误地归因。

/// 一次系统资源占用采样。
///
/// 所有字段都是可选的：某一项采样源缺失时，只有那一项为 `None`，
/// 其余项照常可用。整体不可用时，请直接用 `None`（见
/// [`crate::Perf::sample_system_usage`]）表达。
///
/// 相等性手工实现（见下方 `PartialEq`）：`f64` 本身不满足 `Eq`，
/// 但采样值里出现 `NaN` 属于「数据异常」而非「两个不同值」，
/// 因此把两个 `NaN` 视为相等，让本类型（以及持有它的
/// [`crate::PerfSnapshot`]）可以继续实现 `Eq`。
#[derive(Debug, Clone, Default)]
pub struct SystemUsage {
    /// CPU 占用百分比，取值 `0.0 ..= 100.0`；无采样源时为 `None`。
    pub cpu_percent: Option<f64>,
    /// GPU 占用百分比，取值 `0.0 ..= 100.0`；无采样源时为 `None`。
    ///
    /// 注意：GPU 占用在多适配器机器上通常需要按适配器拆分统计，
    /// 本结构承载的是「整机/主适配器汇总值」，由采样源决定口径。
    pub gpu_percent: Option<f64>,
    /// 已用内存字节数；无采样源时为 `None`。
    pub memory_used_bytes: Option<u64>,
    /// 内存总量字节数；无采样源时为 `None`。
    pub memory_total_bytes: Option<u64>,
}

impl PartialEq for SystemUsage {
    fn eq(&self, other: &Self) -> bool {
        /// `Option<f64>` 相等性：`None` 只与 `None` 相等；`NaN` 与 `NaN` 相等。
        fn opt_eq(a: Option<f64>, b: Option<f64>) -> bool {
            match (a, b) {
                (None, None) => true,
                (Some(x), Some(y)) => x == y || (x.is_nan() && y.is_nan()),
                _ => false,
            }
        }
        opt_eq(self.cpu_percent, other.cpu_percent)
            && opt_eq(self.gpu_percent, other.gpu_percent)
            && self.memory_used_bytes == other.memory_used_bytes
            && self.memory_total_bytes == other.memory_total_bytes
    }
}

impl Eq for SystemUsage {}

impl SystemUsage {
    /// 是否至少有一项可用数据。
    pub fn has_data(&self) -> bool {
        self.cpu_percent.is_some()
            || self.gpu_percent.is_some()
            || self.memory_used_bytes.is_some()
            || self.memory_total_bytes.is_some()
    }

    /// 已用内存占内存总量的比例，取值 `0.0 ..= 1.0`。
    ///
    /// 缺少「已用」或「总量」任一项时返回 `None`（不猜测）。
    pub fn memory_fraction(&self) -> Option<f64> {
        let used = self.memory_used_bytes? as f64;
        let total = self.memory_total_bytes? as f64;
        if total <= 0.0 {
            return None;
        }
        Some(used / total)
    }
}

/// 系统资源占用采样源。
///
/// 实现方负责真正去操作系统（进程 CPU 时间、GPU 负载、内存工作集等）。
/// 采样不可用时返回 `None`，**不要返回伪造数值**。
///
/// 采样源会被跨线程共享，因此要求 `Send + Sync`。
pub trait SystemUsageSource: Send + Sync {
    /// 采样一次系统资源占用。
    ///
    /// 返回 `None` 表示「本次采样不可用」（例如平台不支持、或读取失败）。
    fn sample(&self) -> Option<SystemUsage>;
}

/// 一个恒定不可用的采样源。
///
/// 用于「显式注册一个不提供数据的源」，语义上与「未注册」等价，
/// 便于测试与接入方在开关系统采样时不必反复调整注册逻辑。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoSystemUsageSource;

impl SystemUsageSource for NoSystemUsageSource {
    fn sample(&self) -> Option<SystemUsage> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_data() {
        let u = SystemUsage::default();
        assert!(!u.has_data());
        assert_eq!(u.memory_fraction(), None);
    }

    #[test]
    fn memory_fraction_needs_both_sides() {
        let u = SystemUsage {
            memory_used_bytes: Some(512),
            memory_total_bytes: Some(1024),
            ..Default::default()
        };
        assert_eq!(u.memory_fraction(), Some(0.5));
        assert!(u.has_data());

        let only_used = SystemUsage {
            memory_used_bytes: Some(512),
            ..Default::default()
        };
        assert_eq!(only_used.memory_fraction(), None);
    }

    #[test]
    fn zero_total_memory_is_not_divided() {
        let u = SystemUsage {
            memory_used_bytes: Some(1),
            memory_total_bytes: Some(0),
            ..Default::default()
        };
        assert_eq!(u.memory_fraction(), None);
    }

    #[test]
    fn null_source_never_produces_data() {
        let src = NoSystemUsageSource;
        assert_eq!(src.sample(), None);
    }

    #[test]
    fn equality_treats_nan_as_equal() {
        // 手工实现的 Eq 需要自反性：NaN 与 NaN 必须相等。
        let a = SystemUsage {
            cpu_percent: Some(f64::NAN),
            ..Default::default()
        };
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a, a);

        // 正常值仍按 f64 语义比较。
        let x = SystemUsage {
            cpu_percent: Some(1.5),
            ..Default::default()
        };
        let y = SystemUsage {
            cpu_percent: Some(1.5),
            ..Default::default()
        };
        assert_eq!(x, y);
        assert_ne!(x, a);
    }
}
