//! 10 分钟环形缓冲（§3.13「10 分钟环形缓冲」）。
//!
//! 按时间窗口保留原始样本，超期样本自动淘汰；用于导出逐点曲线。

use std::collections::VecDeque;

use crate::stage::Stage;

/// 默认保留时长（秒，§3.13：10 分钟）。
pub const DEFAULT_WINDOW_SECS: u64 = 600;

/// 默认最大样本数（防止内存无界增长）。
pub const DEFAULT_CAPACITY: usize = 65_536;

/// 一个采样点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// 采样时刻，微秒（单调时钟）。
    pub ts_us: u64,
    /// 阶段。
    pub stage: Stage,
    /// 耗时，纳秒。
    pub value_ns: u64,
}

/// 时间窗口环形缓冲。
#[derive(Debug, Clone)]
pub struct RingBuffer {
    samples: VecDeque<Sample>,
    window_us: u64,
    capacity: usize,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SECS, DEFAULT_CAPACITY)
    }
}

impl RingBuffer {
    /// 新建环形缓冲（窗口秒数 + 最大容量）。
    pub fn new(window_secs: u64, capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(1024),
            window_us: window_secs.saturating_mul(1_000_000),
            capacity: capacity.max(1),
        }
    }

    /// 写入一个采样点，并淘汰超期样本。
    pub fn push(&mut self, sample: Sample) {
        self.samples.push_back(sample);
        self.evict(sample.ts_us);
        while self.samples.len() > self.capacity {
            self.samples.pop_front();
        }
    }

    fn evict(&mut self, now_us: u64) {
        while let Some(front) = self.samples.front() {
            if now_us.saturating_sub(front.ts_us) > self.window_us {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// 全部样本（按时间升序）。
    pub fn samples(&self) -> impl Iterator<Item = &Sample> {
        self.samples.iter()
    }

    /// 样本数量。
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_old_samples() {
        let mut rb = RingBuffer::new(10, 1000);
        rb.push(Sample {
            ts_us: 0,
            stage: Stage::Encode,
            value_ns: 1,
        });
        rb.push(Sample {
            ts_us: 20_000_000,
            stage: Stage::Encode,
            value_ns: 1,
        });
        assert_eq!(rb.len(), 1, "超过窗口的样本应被淘汰");
    }

    #[test]
    fn respects_capacity() {
        let mut rb = RingBuffer::new(600, 4);
        for i in 0..10 {
            rb.push(Sample {
                ts_us: i,
                stage: Stage::Encode,
                value_ns: i as u64,
            });
        }
        assert_eq!(rb.len(), 4);
    }
}
