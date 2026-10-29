//! NTP 式四时间戳时钟同步（§3.13「E2E 测量：NTP 式四时间戳时钟同步（每 5 s）」）。
//!
//! §6 约定「所有时间戳为单调时钟微秒 `u64`；主机抓屏时刻为帧的 capture_ts；
//! 客户端通过 ClockSync 求偏移」。主机与客户端各有自己的单调时钟，只有先求出
//! 两条时钟之间的偏移，才能把主机时间轴上的事件换算到客户端时间轴。
//!
//! ## 四时间戳
//!
//! | 时间戳 | 含义 | 所在时钟 |
//! |---|---|---|
//! | `t0` | 客户端发出同步请求 | 客户端 |
//! | `t1` | 服务器收到同步请求 | 服务器 |
//! | `t2` | 服务器发出同步应答 | 服务器 |
//! | `t3` | 客户端收到同步应答 | 客户端 |
//!
//! ## 估计量
//!
//! - 往返时延：`rtt = (t3 - t0) - (t2 - t1)`；
//! - 时钟偏移：`offset = ((t1 - t0) + (t2 - t3)) / 2`，
//!   定义为 **服务器时钟 - 客户端时钟**（客户端时间加 `offset` 得到服务器时间）；
//! - 单程时延估计：`delay = rtt / 2`（对称假设）。
//!
//! 非对称链路会让 `offset` 带上 `(d_fwd - d_back) / 2` 的系统性偏差，
//! 这是 NTP 单样本法的固有误差：多次采样取中位数可以削弱它，但无法消除。
//! 需要更准的偏移时，应在多路径上采样并取加权中位数。
//!
//! ## 调度
//!
//! [`ClockSyncTracker`] 负责「每 5 s 一次」的节奏（§3.13）。

use std::time::Duration;

/// 同步间隔：每 5 s 一次（§3.13）。
pub const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(5);

/// 同步间隔（微秒），与 [`DEFAULT_SYNC_INTERVAL`] 对应。
pub const DEFAULT_SYNC_INTERVAL_US: u64 = 5_000_000;

/// 一次四时间戳交换。
///
/// 字段顺序与 §3.13 / NTP 定义一致：`t0` 客户端发、`t1` 服务器收、
/// `t2` 服务器发、`t3` 客户端收。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSyncSample {
    /// 客户端发出同步请求的时刻（客户端时钟，微秒）。
    pub t0_us: u64,
    /// 服务器收到同步请求的时刻（服务器时钟，微秒）。
    pub t1_us: u64,
    /// 服务器发出同步应答的时刻（服务器时钟，微秒）。
    pub t2_us: u64,
    /// 客户端收到同步应答的时刻（客户端时钟，微秒）。
    pub t3_us: u64,
}

impl ClockSyncSample {
    /// 由四个时间戳构造。
    pub const fn new(t0_us: u64, t1_us: u64, t2_us: u64, t3_us: u64) -> Self {
        Self {
            t0_us,
            t1_us,
            t2_us,
            t3_us,
        }
    }

    /// 用本次交换估计时钟偏移与往返时延。
    ///
    /// 时间戳不自洽（客户端时钟倒退、服务器时钟倒退、或算出的 RTT 为负）时
    /// 返回 `None`，**不返回伪造的估计值**。
    pub fn estimate(self) -> Option<ClockSyncEstimate> {
        estimate_offset(self.t0_us, self.t1_us, self.t2_us, self.t3_us)
    }
}

/// 一次时钟同步的估计结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSyncEstimate {
    /// 往返时延，微秒。
    pub rtt_us: u64,
    /// 时钟偏移，微秒，定义为「服务器时钟 - 客户端时钟」。
    ///
    /// 客户端时间 `c` 对应的服务器时间是 `c + offset_us`；反之服务器时间
    /// `s` 对应的客户端时间是 `s - offset_us`。偏移可以为负。
    pub offset_us: i64,
    /// 单程时延估计，微秒（`rtt_us / 2`，基于对称链路假设）。
    pub one_way_delay_us: u64,
}

impl ClockSyncEstimate {
    /// 客户端时钟时间 → 服务器时钟时间。
    pub fn client_to_server(&self, client_ts_us: u64) -> i64 {
        client_ts_us as i64 + self.offset_us
    }

    /// 服务器时钟时间 → 客户端时钟时间。
    pub fn server_to_client(&self, server_ts_us: u64) -> i64 {
        server_ts_us as i64 - self.offset_us
    }
}

/// 由四个时间戳估计时钟偏移与往返时延（纯函数）。
///
/// 时间戳含义见 [`ClockSyncSample`]。返回 `None` 表示时间戳不自洽，
/// 此时**不应当**使用任何偏移估计。
pub fn estimate_offset(
    t0_us: u64,
    t1_us: u64,
    t2_us: u64,
    t3_us: u64,
) -> Option<ClockSyncEstimate> {
    // 用 i128 承载差值：t1/t2 在服务器时钟、t0/t3 在客户端时钟，
    // 跨时钟相减可能为负，也可能超出 i64 的中间结果范围。
    let t0 = t0_us as i128;
    let t1 = t1_us as i128;
    let t2 = t2_us as i128;
    let t3 = t3_us as i128;

    // 客户端时钟倒退：样本不可信。
    if t3 < t0 {
        return None;
    }
    // 服务器时钟倒退：样本不可信。
    if t2 < t1 {
        return None;
    }

    // rtt = (t3 - t0) - (t2 - t1)
    let rtt = (t3 - t0) - (t2 - t1);
    if rtt < 0 {
        // 服务器端耗时超过了客户端观测到的往返时间，样本不可信。
        return None;
    }

    // offset = ((t1 - t0) + (t2 - t3)) / 2
    let offset_sum = (t1 - t0) + (t2 - t3);
    let offset = div_round_nearest(offset_sum, 2);

    Some(ClockSyncEstimate {
        rtt_us: rtt as u64,
        offset_us: offset as i64,
        one_way_delay_us: (rtt / 2) as u64,
    })
}

/// 按绝对值四舍五入（0.5 远离零）做整数除法，用于避免负偏移被截断向零。
fn div_round_nearest(value: i128, divisor: i128) -> i128 {
    let half = divisor / 2;
    if value >= 0 {
        (value + half) / divisor
    } else {
        -((-value + half) / divisor)
    }
}

/// 时钟同步调度器：维护「上一次同步时刻」，按固定节奏提示是否需要再同步。
///
/// §3.13 要求「每 5 s 一次」。跟踪器自身不做 I/O，也不发送任何报文，
/// 只负责回答「现在该不该再同步一次」并记住最近一次的估计结果。
#[derive(Debug, Clone)]
pub struct ClockSyncTracker {
    interval_us: u64,
    last_sync_us: Option<u64>,
    last_estimate: Option<ClockSyncEstimate>,
}

impl Default for ClockSyncTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockSyncTracker {
    /// 新建跟踪器，间隔为 [`DEFAULT_SYNC_INTERVAL`]（5 s）。
    pub fn new() -> Self {
        Self::with_interval(DEFAULT_SYNC_INTERVAL)
    }

    /// 新建跟踪器，自定义同步间隔。
    pub fn with_interval(interval: Duration) -> Self {
        Self {
            interval_us: interval.as_micros().min(u64::MAX as u128) as u64,
            last_sync_us: None,
            last_estimate: None,
        }
    }

    /// 同步间隔，微秒。
    pub fn interval_us(&self) -> u64 {
        self.interval_us
    }

    /// 是否到了该做下一次同步的时刻。
    ///
    /// `now_us` 必须是**客户端时钟**的单调微秒时间戳（与 `t0`/`t3` 同一时钟）。
    /// 从未同步过时恒为 `true`；时钟倒退时同样返回 `true`（立刻重新同步）。
    pub fn should_sync(&self, now_us: u64) -> bool {
        match self.last_sync_us {
            None => true,
            Some(last) => {
                if now_us < last {
                    // 时钟倒退：时间基准不可信，立即重新同步。
                    true
                } else {
                    now_us - last >= self.interval_us
                }
            }
        }
    }

    /// 记录一次四时间戳交换的结果。
    ///
    /// 交换成功（时间戳自洽）时更新最近一次估计；样本不可信时保留上一次的估计，
    /// 但仍然推进「上次同步时刻」，避免坏样本导致同步节奏被打乱成忙等。
    ///
    /// 返回本次交换得到的估计值；样本不可信时返回 `None`。
    pub fn update(
        &mut self,
        t0_us: u64,
        t1_us: u64,
        t2_us: u64,
        t3_us: u64,
    ) -> Option<ClockSyncEstimate> {
        let estimate = estimate_offset(t0_us, t1_us, t2_us, t3_us);
        // t3 是客户端时钟上「本次同步完成」的时刻，正是调度所需的时间基准。
        self.last_sync_us = Some(t3_us);
        if estimate.is_some() {
            self.last_estimate = estimate;
        }
        estimate
    }

    /// 与 [`ClockSyncTracker::update`] 相同，但由调用方指定「本次同步完成」的
    /// 客户端时钟时刻（当 `t3` 不是可靠的完成时刻时使用）。
    pub fn update_at(
        &mut self,
        now_us: u64,
        t0_us: u64,
        t1_us: u64,
        t2_us: u64,
        t3_us: u64,
    ) -> Option<ClockSyncEstimate> {
        let estimate = estimate_offset(t0_us, t1_us, t2_us, t3_us);
        self.last_sync_us = Some(now_us);
        if estimate.is_some() {
            self.last_estimate = estimate;
        }
        estimate
    }

    /// 最近一次有效的时钟偏移估计，微秒（服务器时钟 - 客户端时钟）。
    pub fn offset_us(&self) -> Option<i64> {
        self.last_estimate.map(|e| e.offset_us)
    }

    /// 最近一次有效的同步估计。
    pub fn last_estimate(&self) -> Option<ClockSyncEstimate> {
        self.last_estimate
    }

    /// 上一次同步完成的客户端时钟时刻，微秒。
    pub fn last_sync_us(&self) -> Option<u64> {
        self.last_sync_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按模型生成四时间戳：
    /// `delta` = 服务器时钟 - 客户端时钟，`fwd`/`back` 为单程时延。
    fn sample(delta_us: i64, fwd_us: u64, back_us: u64, base_us: u64) -> ClockSyncSample {
        let t0 = base_us;
        let t1 = (base_us as i128 + fwd_us as i128 + delta_us as i128) as u64;
        let t2 = t1;
        let t3 = base_us + fwd_us + back_us;
        ClockSyncSample::new(t0, t1, t2, t3)
    }

    #[test]
    fn symmetric_delay_yields_zero_offset() {
        let s = sample(0, 10_000, 10_000, 1_000_000);
        let e = s.estimate().expect("应当能估计");
        assert_eq!(e.rtt_us, 20_000);
        assert_eq!(e.offset_us, 0);
        assert_eq!(e.one_way_delay_us, 10_000);
    }

    #[test]
    fn asymmetric_delay_biases_offset_by_half_difference() {
        // 上行 5 ms、下行 15 ms：偏移估计偏差 = (5 - 15) / 2 = -5 ms。
        let s = sample(0, 5_000, 15_000, 1_000_000);
        let e = s.estimate().expect("应当能估计");
        assert_eq!(e.rtt_us, 20_000);
        assert_eq!(e.offset_us, -5_000);
    }

    #[test]
    fn negative_offset_is_reported_with_sign() {
        // 服务器时钟比客户端慢 250 ms。
        let s = sample(-250_000, 10_000, 10_000, 1_000_000);
        let e = s.estimate().expect("应当能估计");
        assert_eq!(e.offset_us, -250_000);
        assert_eq!(e.rtt_us, 20_000);
    }

    #[test]
    fn positive_offset_is_reported_with_sign() {
        // 服务器时钟比客户端快 40 ms。
        let s = sample(40_000, 10_000, 10_000, 1_000_000);
        let e = s.estimate().expect("应当能估计");
        assert_eq!(e.offset_us, 40_000);
    }

    #[test]
    fn large_rtt_is_estimated() {
        // RTT 2 s。
        let s = sample(0, 1_000_000, 1_000_000, 1_000_000);
        let e = s.estimate().expect("应当能估计");
        assert_eq!(e.rtt_us, 2_000_000);
        assert_eq!(e.one_way_delay_us, 1_000_000);
    }

    #[test]
    fn backwards_client_clock_is_rejected() {
        // t3 < t0：客户端时钟倒退，样本不可信。
        let s = ClockSyncSample::new(2_000_000, 2_000_000, 2_000_000, 1_000_000);
        assert_eq!(s.estimate(), None);
    }

    #[test]
    fn backwards_server_clock_is_rejected() {
        // t2 < t1：服务器时钟倒退，样本不可信。
        let s = ClockSyncSample::new(1_000_000, 2_000_000, 1_500_000, 1_100_000);
        assert_eq!(s.estimate(), None);
    }

    #[test]
    fn negative_rtt_is_rejected() {
        // 服务器端耗时超过客户端观测到的往返时间，样本不可信。
        let s = ClockSyncSample::new(1_000_000, 1_000_000, 2_000_000, 1_100_000);
        assert_eq!(s.estimate(), None);
    }

    #[test]
    fn offset_converts_between_clocks() {
        let s = sample(40_000, 10_000, 10_000, 1_000_000);
        let e = s.estimate().expect("应当能估计");
        let server_ts = e.client_to_server(1_000_000);
        assert_eq!(server_ts, 1_040_000);
        assert_eq!(e.server_to_client(1_040_000), 1_000_000);
    }

    #[test]
    fn tracker_requests_first_sync_immediately() {
        let t = ClockSyncTracker::new();
        assert!(t.should_sync(0));
        assert_eq!(t.last_sync_us(), None);
        assert_eq!(t.offset_us(), None);
    }

    #[test]
    fn tracker_respects_five_second_interval() {
        let mut t = ClockSyncTracker::new();
        assert_eq!(t.interval_us(), 5_000_000);
        let s = sample(0, 10_000, 10_000, 1_000_000);
        let t3 = s.t3_us;
        t.update(s.t0_us, s.t1_us, s.t2_us, s.t3_us);
        assert!(!t.should_sync(t3));
        assert!(!t.should_sync(t3 + 4_999_999));
        assert!(t.should_sync(t3 + 5_000_000));
    }

    #[test]
    fn tracker_keeps_last_good_estimate_on_bad_sample() {
        let mut t = ClockSyncTracker::new();
        let good = sample(40_000, 10_000, 10_000, 1_000_000);
        t.update(good.t0_us, good.t1_us, good.t2_us, good.t3_us);
        assert_eq!(t.offset_us(), Some(40_000));

        // 坏样本：客户端时钟倒退。
        let bad = ClockSyncSample::new(2_000_000, 2_000_000, 2_000_000, 1_000_000);
        assert_eq!(t.update(bad.t0_us, bad.t1_us, bad.t2_us, bad.t3_us), None);
        assert_eq!(t.offset_us(), Some(40_000), "坏样本不应覆盖上一次估计");
    }

    #[test]
    fn tracker_syncs_again_when_clock_goes_backwards() {
        let mut t = ClockSyncTracker::new();
        let s = sample(0, 10_000, 10_000, 1_000_000);
        t.update(s.t0_us, s.t1_us, s.t2_us, s.t3_us);
        assert!(t.should_sync(0), "时钟倒退时应立即重新同步");
    }
}
