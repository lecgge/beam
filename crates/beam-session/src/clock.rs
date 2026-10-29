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
}
