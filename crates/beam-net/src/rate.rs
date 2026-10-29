//! 码率控制（§3.2「基于丢包与 RTT 的 GCC 简化版」）。
//!
//! 规则（GCC 简化）：
//! - 丢包率 EWMA > 2% → 降码率 12.5%；
//! - 丢包率 EWMA < 1% 且 RTT 稳定 → 升码率 6.25%；
//! - 其余保持。
//!
//! 上下限取 [`crate::config::NetConfig`] 的 `bitrate_min` / `bitrate_max`。

use std::time::Duration;

use beam_proto::Feedback;

/// 码率调整方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateAction {
    /// 升码率。
    Increase,
    /// 降码率。
    Decrease,
    /// 保持不变。
    Hold,
}

/// 一次反馈后的码率决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitrateDecision {
    /// 调整方向。
    pub action: RateAction,
    /// 目标码率，bit/s。
    pub target_bps: u32,
    /// 决策原因（仅用于日志）。
    pub reason: &'static str,
}

/// 丢包率 EWMA 超过该值即降码率（2%）。
pub const LOSS_DOWN_PPM: u32 = 200;

/// 丢包率 EWMA 低于该值才考虑升码率（1%）。
pub const LOSS_UP_PPM: u32 = 100;

/// RTT 相对基准增长超过该比例即暂停升码率。
pub const RTT_GROWTH_LIMIT_PCT: u32 = 20;

/// 降码率步长（1/8）。
const DOWN_NUM: u64 = 7;
/// 降码率分母（1/8）。
const DOWN_DEN: u64 = 8;
/// 升码率步长（1/16）。
const UP_NUM: u64 = 17;
/// 升码率分母（1/16）。
const UP_DEN: u64 = 16;

/// 码率控制器。
#[derive(Debug, Clone)]
pub struct RateController {
    current_bps: u32,
    min_bps: u32,
    max_bps: u32,
    loss_ewma: u32,
    rtt_ewma: Duration,
    rtt_baseline: Option<Duration>,
    samples: u32,
}

impl RateController {
    /// 用码率上下限构造，初始码率取下限。
    pub fn new(min_bps: u32, max_bps: u32) -> Self {
        let min_bps = min_bps.max(1);
        let max_bps = max_bps.max(min_bps);
        Self {
            current_bps: min_bps,
            min_bps,
            max_bps,
            loss_ewma: 0,
            rtt_ewma: Duration::ZERO,
            rtt_baseline: None,
            samples: 0,
        }
    }

    /// 当前码率。
    pub fn current_bitrate(&self) -> u32 {
        self.current_bps
    }

    /// 码率下限。
    pub fn min_bitrate(&self) -> u32 {
        self.min_bps
    }

    /// 码率上限。
    pub fn max_bitrate(&self) -> u32 {
        self.max_bps
    }

    /// 处理一次网络反馈，返回码率决策。
    pub fn on_feedback(&mut self, fb: &Feedback) -> BitrateDecision {
        // 丢包率 EWMA（8 样本）；首个样本直接作为初值，避免从 0 爬升导致误判。
        self.loss_ewma = if self.samples == 0 {
            fb.loss
        } else {
            (self.loss_ewma as u64 * 7 + fb.loss as u64) as u32 / 8
        };
        self.samples += 1;
        // RTT EWMA。
        let rtt = fb.rtt;
        self.rtt_ewma = if self.rtt_ewma.is_zero() {
            rtt
        } else {
            (self.rtt_ewma * 3 + rtt) / 4
        };
        let baseline = *self.rtt_baseline.get_or_insert(self.rtt_ewma);

        if self.loss_ewma > LOSS_DOWN_PPM {
            let next = ((self.current_bps as u64 * DOWN_NUM) / DOWN_DEN) as u32;
            let next = next.clamp(self.min_bps, self.max_bps);
            let decreased = next < self.current_bps;
            self.current_bps = next;
            return BitrateDecision {
                action: if decreased {
                    RateAction::Decrease
                } else {
                    RateAction::Hold
                },
                target_bps: self.current_bps,
                reason: "丢包率偏高，降码率",
            };
        }

        let rtt_grown = baseline > Duration::ZERO
            && self.rtt_ewma > baseline + baseline / 4 * (RTT_GROWTH_LIMIT_PCT as u32 / 100);
        if self.loss_ewma < LOSS_UP_PPM && !rtt_grown {
            let next = ((self.current_bps as u64 * UP_NUM) / UP_DEN) as u32;
            let next = next.clamp(self.min_bps, self.max_bps);
            let increased = next > self.current_bps;
            self.current_bps = next;
            return BitrateDecision {
                action: if increased {
                    RateAction::Increase
                } else {
                    RateAction::Hold
                },
                target_bps: self.current_bps,
                reason: "链路良好，升码率",
            };
        }

        BitrateDecision {
            action: RateAction::Hold,
            target_bps: self.current_bps,
            reason: "保持当前码率",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(loss: u32, rtt_ms: u64) -> Feedback {
        Feedback {
            loss,
            rtt: Duration::from_millis(rtt_ms),
            recv_rate: 30_000_000,
        }
    }

    #[test]
    fn loss_above_two_percent_decreases() {
        let mut rc = RateController::new(1_000_000, 100_000_000);
        // 先把码率升上去。
        for _ in 0..40 {
            rc.on_feedback(&fb(10, 5));
        }
        let before = rc.current_bitrate();
        assert!(before > 1_000_000);
        let d = rc.on_feedback(&fb(5_000, 5));
        assert_eq!(d.action, RateAction::Decrease);
        assert!(d.target_bps < before);
    }

    #[test]
    fn low_loss_increases_and_clamps() {
        let mut rc = RateController::new(1_000_000, 2_000_000);
        let d = rc.on_feedback(&fb(0, 5));
        assert_eq!(d.action, RateAction::Increase);
        assert!(d.target_bps <= 2_000_000);
    }

    #[test]
    fn bitrate_never_exceeds_bounds() {
        let mut rc = RateController::new(1_000_000, 3_000_000);
        for _ in 0..100 {
            let d = rc.on_feedback(&fb(0, 5));
            assert!(d.target_bps >= 1_000_000 && d.target_bps <= 3_000_000);
        }
    }

    #[test]
    fn holds_when_loss_is_middle() {
        let mut rc = RateController::new(1_000_000, 100_000_000);
        let d = rc.on_feedback(&fb(150, 5));
        assert_eq!(d.action, RateAction::Hold);
        assert_eq!(d.target_bps, rc.current_bitrate());
    }
}
