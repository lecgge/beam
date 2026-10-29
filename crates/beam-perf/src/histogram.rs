//! 耗时直方图（§3.13「各阶段耗时直方图」）。
//!
//! 用固定桶计数，单点采集开销 < 200 ns（§3.13 性能预算），
//! 因此不做排序、不保存原始样本，只维护计数与极值。

use std::time::Duration;

/// 直方图桶边界（纳秒）。
pub const BUCKET_EDGES_NS: [u64; 10] = [
    50_000,     // 50 µs
    100_000,    // 100 µs
    250_000,    // 250 µs
    500_000,    // 500 µs
    1_000_000,  // 1 ms
    2_000_000,  // 2 ms
    5_000_000,  // 5 ms
    10_000_000, // 10 ms
    20_000_000, // 20 ms
    50_000_000, // 50 ms
];

/// 单阶段耗时直方图。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Histogram {
    /// 各桶计数（最后一个桶为溢出桶）。
    pub buckets: [u64; BUCKET_EDGES_NS.len() + 1],
    /// 样本总数。
    pub count: u64,
    /// 累计耗时，纳秒。
    pub total_ns: u64,
    /// 最小耗时，纳秒。
    pub min_ns: u64,
    /// 最大耗时，纳秒。
    pub max_ns: u64,
}

impl Histogram {
    /// 空直方图。
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一个样本。
    pub fn record(&mut self, d: Duration) {
        let ns = d.as_nanos().min(u64::MAX as u128) as u64;
        self.count += 1;
        self.total_ns = self.total_ns.saturating_add(ns);
        if self.count == 1 {
            self.min_ns = ns;
            self.max_ns = ns;
        } else {
            self.min_ns = self.min_ns.min(ns);
            self.max_ns = self.max_ns.max(ns);
        }
        let mut idx = BUCKET_EDGES_NS.len();
        for (i, edge) in BUCKET_EDGES_NS.iter().enumerate() {
            if ns < *edge {
                idx = i;
                break;
            }
        }
        self.buckets[idx] += 1;
    }

    /// 平均耗时。
    pub fn mean(&self) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }
        Duration::from_nanos(self.total_ns / self.count)
    }

    /// 近似分位数（按桶线性插值）。
    pub fn percentile(&self, p: f64) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }
        let target = ((self.count as f64) * p.clamp(0.0, 1.0)).ceil() as u64;
        let mut acc = 0u64;
        for (i, c) in self.buckets.iter().enumerate() {
            acc += c;
            if acc >= target {
                let lower = if i == 0 { 0 } else { BUCKET_EDGES_NS[i - 1] };
                let upper = if i < BUCKET_EDGES_NS.len() {
                    BUCKET_EDGES_NS[i]
                } else {
                    BUCKET_EDGES_NS[BUCKET_EDGES_NS.len() - 1]
                };
                return Duration::from_nanos(lower + (upper - lower) / 2);
            }
        }
        Duration::from_nanos(self.max_ns)
    }

    /// p50。
    pub fn p50(&self) -> Duration {
        self.percentile(0.50)
    }

    /// p95。
    pub fn p95(&self) -> Duration {
        self.percentile(0.95)
    }

    /// p99。
    pub fn p99(&self) -> Duration {
        self.percentile(0.99)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram_is_zero() {
        let h = Histogram::new();
        assert_eq!(h.count, 0);
        assert_eq!(h.mean(), Duration::ZERO);
        assert_eq!(h.p99(), Duration::ZERO);
    }

    #[test]
    fn records_update_counters() {
        let mut h = Histogram::new();
        h.record(Duration::from_millis(1));
        h.record(Duration::from_millis(3));
        assert_eq!(h.count, 2);
        assert_eq!(h.min_ns, 1_000_000);
        assert_eq!(h.max_ns, 3_000_000);
        assert_eq!(h.mean(), Duration::from_millis(2));
    }

    #[test]
    fn percentiles_are_ordered() {
        let mut h = Histogram::new();
        for i in 1..=100 {
            h.record(Duration::from_micros(i * 10));
        }
        assert!(h.p50() <= h.p95());
        assert!(h.p95() <= h.p99());
    }
}
