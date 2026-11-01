//! 会话时钟（§3.9「`stall_threshold` / `reconnect_timeout` 必须有判定」）。
//!
//! §4.1 要求「3 s 无数据 → Reconnecting」「30 s 内恢复 → Streaming / 超时 → Closed」。
//! 这两个判定都依赖时间，因此时钟必须**可注入**：
//! 单测用 [`ManualClock`] 推进时间即可覆盖状态迁移，不必真的 sleep 30 秒。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// 会话时钟（单调毫秒）。
pub trait Clock: Send + Sync {
    /// 当前单调时间戳（毫秒）。
    ///
    /// 只保证同一进程内单调递增；零点由实现自行决定。
    fn now_ms(&self) -> u64;

    /// 当前单调时间戳（**微秒**）。
    ///
    /// §5：「所有时间戳为单调时钟微秒 `u64`」——
    /// `ClockSync` 的四个时间戳、`capture_ts`、E2E 延迟都用这个口径。
    ///
    /// 默认实现由 [`Clock::now_ms`] 换算（毫秒精度），
    /// 需要微秒精度的实现（生产路径的 [`SystemClock`]）应当覆写它。
    fn now_us(&self) -> u64 {
        self.now_ms().saturating_mul(1000)
    }
}

/// 时钟同步结果（§5「客户端通过 ClockSync 求偏移」）。
///
/// 有了它，客户端才能把主机的 `capture_ts`（主机时钟）换算到自己的时钟，
/// 从而算出真实的端到端延迟（§3.13 的 E2E 阶段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClockSyncState {
    /// **主机时钟 − 客户端时钟**（微秒）。
    ///
    /// 正值表示主机的时钟比客户端快：客户端收到帧时，
    /// 主机那边的「抓屏时刻」换算到客户端时钟要**加上**这个偏移。
    pub offset_us: i64,
    /// 往返时延（微秒）。
    pub rtt_us: u64,
    /// 测量时刻（客户端时钟，微秒）。
    pub measured_at_us: u64,
}

/// 由四时间戳估算时钟偏移与往返时延（NTP 式，§5）。
///
/// ```text
/// offset = ((t1 - t0) + (t2 - t3)) / 2      // 主机时钟 − 客户端时钟
/// rtt    = (t3 - t0) - (t2 - t1)            // 往返时延
/// ```
///
/// - `t0`：客户端发送时刻；`t1`：主机接收时刻；
/// - `t2`：主机回复时刻；`t3`：客户端接收时刻。
/// - 全部是**微秒**（§5）。
///
/// 用 `i128` 运算，避免时间戳相减时溢出/回绕。
pub fn estimate_offset(t0: u64, t1: u64, t2: u64, t3: u64) -> (i64, u64) {
    let offset = ((t1 as i128 - t0 as i128) + (t2 as i128 - t3 as i128)) / 2;
    let rtt = (t3 as i128 - t0 as i128) - (t2 as i128 - t1 as i128);
    (offset as i64, rtt.max(0) as u64)
}

/// 系统单调时钟（生产路径）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        let start = START.get_or_init(Instant::now);
        start.elapsed().as_millis() as u64
    }

    fn now_us(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        let start = START.get_or_init(Instant::now);
        start.elapsed().as_micros() as u64
    }
}

/// 手动时钟（测试路径）。
///
/// 用 [`ManualClock::advance`] 推进时间，用于覆盖 §4.1 的两个超时判定。
#[derive(Debug, Clone, Default)]
pub struct ManualClock {
    now_ms: Arc<AtomicU64>,
}

impl ManualClock {
    /// 新建手动时钟（从 0 ms 开始）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 以指定时间点新建手动时钟。
    pub fn starting_at(now_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(now_ms)),
        }
    }

    /// 推进时间。
    pub fn advance(&self, delta: Duration) {
        self.now_ms
            .fetch_add(delta.as_millis() as u64, Ordering::SeqCst);
    }

    /// 直接设置当前时间。
    pub fn set_ms(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// 把时长换算成毫秒数（向上取整到 1 ms，避免零阈值被当成「立即超时」）。
pub(crate) fn duration_ms(d: Duration) -> u64 {
    let ms = d.as_millis();
    if ms == 0 && !d.is_zero() {
        1
    } else {
        ms as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_advances() {
        let clock = ManualClock::new();
        assert_eq!(clock.now_ms(), 0);
        clock.advance(Duration::from_secs(3));
        assert_eq!(clock.now_ms(), 3000);
        clock.set_ms(10);
        assert_eq!(clock.now_ms(), 10);
    }

    #[test]
    fn system_clock_is_monotonic() {
        let clock = SystemClock;
        let a = clock.now_ms();
        let b = clock.now_ms();
        assert!(b >= a, "系统时钟必须单调不减");
    }

    #[test]
    fn duration_ms_rounds_up_sub_millisecond_values() {
        assert_eq!(duration_ms(Duration::from_secs(3)), 3000);
        assert_eq!(duration_ms(Duration::ZERO), 0);
        assert_eq!(duration_ms(Duration::from_nanos(1)), 1);
    }

    #[cfg(test)]
    #[test]
    fn offset_is_the_midpoint_of_the_two_one_way_delays() {
        // 客户端时钟 2.000 s，主机快 1 s ⇒ 偏移 +1 s，往返无延迟。
        let (offset_us, rtt_us) = estimate_offset(2_000_000, 3_000_000, 3_000_000, 2_000_000);
        assert_eq!(offset_us, 1_000_000);
        assert_eq!(rtt_us, 0);
    }

    #[test]
    fn negative_offset_means_the_host_clock_is_behind() {
        // 主机慢 1 s ⇒ 偏移 −1 s。
        let (offset_us, rtt_us) = estimate_offset(2_000_000, 1_000_000, 1_000_000, 2_000_000);
        assert_eq!(offset_us, -1_000_000);
        assert_eq!(rtt_us, 0);
    }

    #[test]
    fn round_trip_time_is_the_wall_clock_span() {
        // 单程各 250 ms ⇒ 往返 500 ms，偏移仍是 0。
        let (offset_us, rtt_us) = estimate_offset(1_000_000, 1_250_000, 1_250_000, 1_500_000);
        assert_eq!(offset_us, 0);
        assert_eq!(rtt_us, 500_000);
    }

    #[test]
    fn asymmetric_path_does_not_bias_the_offset_by_the_full_amount() {
        // 去程 100 ms（t0→t1）、回程 300 ms（t2→t3）：偏移应落在中间（−100 ms），
        // 而不是 −200 ms；往返时延就是墙钟跨度 400 ms。
        let (offset_us, rtt_us) = estimate_offset(0, 100_000, 100_000, 400_000);
        assert_eq!(offset_us, -100_000);
        assert_eq!(rtt_us, 400_000);
    }
}
