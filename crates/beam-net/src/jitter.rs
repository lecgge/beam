//! 接收端抖动缓冲（§3.2 配置 `jitter_frames`、§3.2 职责「接收端抖动缓冲」）。
//!
//! ## 为什么需要
//!
//! §3.2 配置项写着 `jitter_frames`（有线 0，Wi-Fi 自适应 0–1）：
//! 有线链路抖动可忽略，缓冲深度 0 即「来一帧放一帧」；
//! Wi-Fi 上偶发的乱序 / 抖动会让画面「后到的帧反而更早显示」，
//! 留 1 帧的缓冲可以把这种乱序吸收掉。
//!
//! ## 语义
//!
//! - 缓冲按 `capture_ts_us` 升序排队，队首是最老的一帧；
//! - 释放条件（满足其一）：
//!   1. 队列长度 **超过** 当前深度（缓冲已满，必须放行以限制延迟）；
//!   2. 队首已经等了 `max_hold_us`（默认约一帧 @60fps）——到期强制放行；
//! - 深度为 0 时条件 1 立刻成立 ⇒ 帧一到就放行，**不增加任何延迟**。
//!
//! ## 自适应（§3.2「Wi-Fi 自适应 0–1」）
//!
//! 深度上限是 [`MAX_JITTER_DEPTH`] = 1，由 `NetConfig::jitter_frames` 决定
//! （有线配 0，Wi-Fi 配 1）。在本缓冲内部再做一层自适应：
//!
//! - **初始深度 = 上限**：`jitter_frames = 1` 时从第一帧起就有 1 帧缓冲；
//!   `jitter_frames = 0` 时上限就是 0，完全不缓冲；
//! - 观察到**乱序到达**（新帧的 `capture_ts_us` 比已收到的最新帧更早）时，
//!   深度恢复到上限；
//! - 连续 [`JITTER_HOLD_US`]（约 1 秒）没有再观察到乱序，深度回落到 0，
//!   恢复最低延迟。
//!
//! 有线（`jitter_frames = 0`）时上限就是 0，自适应再怎么抖也不会缓冲，
//! 因此这条配置项在两种链路上都「真的生效」。

use std::collections::VecDeque;
use std::time::Duration;

use beam_proto::AssembledFrame;

/// 单帧在抖动缓冲里最多停留的时间（微秒）：约一帧 @60fps。
pub const DEFAULT_MAX_HOLD_US: u64 = 16_667;

/// §3.2「Wi-Fi 自适应 0–1」：抖动缓冲深度的上限。
pub const MAX_JITTER_DEPTH: usize = 1;

/// 观察到抖动后维持深度的时长（微秒）：约 1 秒，之后回落到 0。
pub const JITTER_HOLD_US: u64 = 1_000_000;

/// 缓冲里的一帧（额外记录本地到达时刻）。
#[derive(Debug)]
struct QueuedFrame {
    /// 本地单调时钟的到达时刻（微秒）。
    arrival_us: u64,
    /// 重组好的画面。
    frame: AssembledFrame,
}

/// 接收端抖动缓冲。
///
/// 深度由 `NetConfig::jitter_frames` 决定，超过 [`MAX_JITTER_DEPTH`] 的部分会被截断
/// （§3.2 规定取值范围是 0–1）。
#[derive(Debug)]
pub struct JitterBuffer {
    /// 深度上限（来自 `jitter_frames`）。
    max_depth: usize,
    /// 当前生效的深度（自适应结果，0 或 1）。
    depth: usize,
    /// 待释放的帧（按 `capture_ts_us` 升序）。
    queue: VecDeque<QueuedFrame>,
    /// 单帧最长停留时间（微秒）。
    max_hold_us: u64,
    /// 最近一次观察到乱序的时刻（本地单调时钟）。
    last_jitter_us: u64,
    /// 观察到的乱序次数（统计用）。
    out_of_order: u64,
}

impl JitterBuffer {
    /// 构造；`max_depth` 来自 `NetConfig::jitter_frames`（0 = 不缓冲）。
    pub fn new(max_depth: usize) -> Self {
        Self::with_max_hold(max_depth, DEFAULT_MAX_HOLD_US)
    }

    /// 构造并指定单帧最长停留时间（微秒）。
    pub fn with_max_hold(max_depth: usize, max_hold_us: u64) -> Self {
        let max_depth = max_depth.min(MAX_JITTER_DEPTH);
        Self {
            max_depth,
            // 初始深度 = 上限：`jitter_frames = 1` 时从第一帧起就有缓冲。
            depth: max_depth,
            queue: VecDeque::new(),
            max_hold_us,
            last_jitter_us: 0,
            out_of_order: 0,
        }
    }

    /// 放入一帧；`now_us` 是本地单调时钟的到达时刻（见 [`crate::now_us`]）。
    pub fn push(&mut self, frame: AssembledFrame, now_us: u64) {
        // 乱序检测：新帧比队尾更早 ⇒ 链路有抖动，深度恢复到上限。
        if let Some(back) = self.queue.back() {
            if frame.capture_ts_us < back.frame.capture_ts_us {
                self.out_of_order += 1;
                self.last_jitter_us = now_us;
                self.depth = self.max_depth;
            }
        }
        // 按 capture_ts 升序插入（队首最老）。
        let pos = self
            .queue
            .iter()
            .position(|q| q.frame.capture_ts_us > frame.capture_ts_us)
            .unwrap_or(self.queue.len());
        self.queue.insert(
            pos,
            QueuedFrame {
                arrival_us: now_us,
                frame,
            },
        );
    }

    /// 取一帧可以释放的画面；暂时没有就返回 `None`。
    ///
    /// `now_us` 是本地单调时钟读数（见 [`crate::now_us`]）。
    pub fn pop(&mut self, now_us: u64) -> Option<AssembledFrame> {
        // 抖动平息后回落到深度 0，恢复最低延迟。
        self.depth = self.effective_depth(now_us);
        let front = self.queue.front()?;
        let overfull = self.queue.len() > self.depth;
        let held_enough = now_us.saturating_sub(front.arrival_us) >= self.max_hold_us;
        if !overfull && !held_enough {
            return None;
        }
        self.queue.pop_front().map(|q| q.frame)
    }

    /// 距离下一帧可以释放还有多久。
    ///
    /// - `None`：缓冲里没有待释放的帧（调用方应当直接等下一个数据报）；
    /// - `Some(Duration::ZERO)`：已经可以放行；
    /// - `Some(d)`：还要再等 `d`。
    pub fn next_release_in(&self, now_us: u64) -> Option<Duration> {
        let front = self.queue.front()?;
        let depth = self.effective_depth(now_us);
        if self.queue.len() > depth {
            return Some(Duration::ZERO);
        }
        let remaining = self
            .max_hold_us
            .saturating_sub(now_us.saturating_sub(front.arrival_us));
        Some(Duration::from_micros(remaining))
    }

    /// 当前应当生效的深度：抖动平息后回落到 0。
    fn effective_depth(&self, now_us: u64) -> usize {
        if self.depth > 0 && now_us.saturating_sub(self.last_jitter_us) > JITTER_HOLD_US {
            0
        } else {
            self.depth
        }
    }

    /// 当前生效的深度（0 或 1）。
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// 深度上限。
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// 缓冲里待释放的帧数。
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 缓冲是否为空。
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 观察到的乱序到达次数。
    pub fn out_of_order_count(&self) -> u64 {
        self.out_of_order
    }
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn frame(id: u32, capture_ts: u64) -> AssembledFrame {
        AssembledFrame {
            frame_id: id,
            pts_us: capture_ts,
            capture_ts_us: capture_ts,
            is_key: false,
            payload: Bytes::from_static(b"x"),
        }
    }

    #[test]
    fn depth_zero_releases_immediately() {
        let mut buf = JitterBuffer::new(0);
        buf.push(frame(1, 100), 1_000);
        let out = buf.pop(1_000).expect("深度 0 应当立即放行");
        assert_eq!(out.frame_id, 1);
        assert!(buf.is_empty());
    }

    #[test]
    fn depth_one_holds_until_full_or_timeout() {
        let mut buf = JitterBuffer::with_max_hold(1, 1_000);
        buf.push(frame(1, 100), 1_000);
        // 只有 1 帧、深度 1 ⇒ 压住不放。
        assert!(buf.pop(1_100).is_none());
        // 等待超过 max_hold_us ⇒ 强制放行。
        let out = buf.pop(2_000).expect("超时后应当放行");
        assert_eq!(out.frame_id, 1);
    }

    #[test]
    fn depth_one_releases_when_second_frame_arrives() {
        let mut buf = JitterBuffer::with_max_hold(1, 1_000_000);
        buf.push(frame(1, 100), 1_000);
        assert!(buf.pop(1_000).is_none());
        buf.push(frame(2, 200), 1_100);
        let out = buf.pop(1_100).expect("缓冲满后应当放行队首");
        assert_eq!(out.frame_id, 1);
        // 深度 1：剩下的那一帧又被压住。
        assert!(buf.pop(1_100).is_none());
        // 等到超时后放行。
        let out = buf.pop(1_100 + 1_000_000).expect("超时后应当放行");
        assert_eq!(out.frame_id, 2);
    }

    #[test]
    fn out_of_order_arrival_raises_depth() {
        let mut buf = JitterBuffer::with_max_hold(1, 1_000_000);
        buf.push(frame(2, 200), 1_000);
        // 后到的帧 capture_ts 更小 ⇒ 判定抖动。
        buf.push(frame(1, 100), 1_100);
        assert_eq!(buf.out_of_order_count(), 1);
        assert_eq!(buf.depth(), 1);
        // 乱序帧排到队首。
        let out = buf.pop(1_100).expect("缓冲满后应当放行");
        assert_eq!(out.frame_id, 1);
    }

    #[test]
    fn depth_relaxes_back_to_zero_after_jitter_window() {
        let mut buf = JitterBuffer::with_max_hold(1, 1_000_000);
        buf.push(frame(2, 200), 1_000);
        buf.push(frame(1, 100), 1_100);
        assert_eq!(buf.depth(), 1);
        // 超过 JITTER_HOLD_US 仍无乱序 ⇒ 深度回落。
        buf.pop(1_100 + JITTER_HOLD_US + 1);
        assert_eq!(buf.depth(), 0);
    }

    #[test]
    fn depth_is_clamped_to_one() {
        let buf = JitterBuffer::new(9);
        assert_eq!(buf.max_depth(), MAX_JITTER_DEPTH);
    }

    #[test]
    fn next_release_in_reports_wait_time() {
        let mut buf = JitterBuffer::with_max_hold(1, 1_000);
        assert!(buf.next_release_in(0).is_none(), "空缓冲没有释放时刻");
        buf.push(frame(1, 100), 5_000);
        let wait = buf.next_release_in(5_000).expect("应当有释放时刻");
        assert_eq!(wait, Duration::from_micros(1_000));
    }
}
