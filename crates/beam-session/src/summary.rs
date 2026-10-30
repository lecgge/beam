//! 会话统计汇总（§3.9「统计汇总」）。
//!
//! ## 为什么要有它
//!
//! §3.11 的 `session://stats` 是**会话内**的周期性指标（500 ms 一拍），
//! 会话一结束就没了；但上层（桌面端 / Android）在会话结束后仍然要能查
//! 「这次串流跑了多久、出了多少帧、丢了多少帧、RTT 多少」——
//! 这就是本模块的 [`SessionSummary`]。
//!
//! ## 数据从哪来
//!
//! - 帧数 / 丢帧数：会话运行体（[`crate::client`] / [`crate::host`]）逐帧累加；
//! - RTT / 丢包率 / 帧率 / 码率：§5 的 `StatsReport` 控制消息；
//! - 重连次数：每次进入 `Reconnecting` 记一次。
//!
//! 汇总**只读不写**：运行体维护计数器，句柄在查询时取快照
//! （见 [`crate::handle::SessionHandle::summary`]）。

use crate::state::SessionState;

/// 会话汇总（§3.9「统计汇总」）。
///
/// 会话结束后依然可以查询：计数器由运行体持有，随句柄一起活到句柄被丢弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionSummary {
    /// 会话号（§5 `Resume{session_id}` 复用同一个值）。
    pub session_id: u64,
    /// 会话时长（毫秒）。
    pub duration_ms: u64,
    /// 已发送（主机）/ 已接收（客户端）的画面帧数。
    pub frames: u64,
    /// 累计丢帧数（§3.2 **E2004**）。
    pub dropped_frames: u64,
    /// 重连次数（进入 `Reconnecting` 的次数）。
    pub reconnects: u32,
    /// 最近一次上报的往返时延（微秒）。
    pub rtt_us: u32,
    /// 最近一次上报的丢包率（百万分比，`0..=1_000_000`）。
    pub loss_ppm: u32,
    /// 最近一次上报的实际帧率。
    pub fps: u32,
    /// 最近一次上报的实际码率（bit/s）。
    pub bitrate_bps: u32,
    /// 会话当前（或结束时的）状态。
    pub state: SessionState,
}

/// 会话统计计数器（会话层内部）。
///
/// 运行体维护它，句柄在查询时取快照——这样「会话结束后仍可查」是自然结果，
/// 不需要在会话结束时额外做一次收尾上报。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SessionCounters {
    /// 会话开始时刻（毫秒，与会话时钟同源）。
    pub(crate) started_ms: u64,
    /// 会话号。
    pub(crate) session_id: u64,
    /// 已发送 / 已接收的帧数。
    pub(crate) frames: u64,
    /// 累计丢帧数。
    pub(crate) dropped_frames: u64,
    /// 重连次数。
    pub(crate) reconnects: u32,
    /// 最近一次上报的往返时延（微秒）。
    pub(crate) rtt_us: u32,
    /// 最近一次上报的丢包率（百万分比）。
    pub(crate) loss_ppm: u32,
    /// 最近一次上报的实际帧率。
    pub(crate) fps: u32,
    /// 最近一次上报的实际码率（bit/s）。
    pub(crate) bitrate_bps: u32,
}

impl SessionCounters {
    /// 取当前时刻的汇总快照。
    pub(crate) fn snapshot(&self, now_ms: u64, state: SessionState) -> SessionSummary {
        SessionSummary {
            session_id: self.session_id,
            duration_ms: now_ms.saturating_sub(self.started_ms),
            frames: self.frames,
            dropped_frames: self.dropped_frames,
            reconnects: self.reconnects,
            rtt_us: self.rtt_us,
            loss_ppm: self.loss_ppm,
            fps: self.fps,
            bitrate_bps: self.bitrate_bps,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reports_duration_and_counters() {
        let counters = SessionCounters {
            started_ms: 1_000,
            session_id: 7,
            frames: 120,
            dropped_frames: 3,
            reconnects: 1,
            rtt_us: 4_000,
            loss_ppm: 12,
            fps: 60,
            bitrate_bps: 20_000_000,
        };
        let summary = counters.snapshot(6_000, SessionState::Closed);
        assert_eq!(summary.session_id, 7);
        assert_eq!(summary.duration_ms, 5_000);
        assert_eq!(summary.frames, 120);
        assert_eq!(summary.dropped_frames, 3);
        assert_eq!(summary.reconnects, 1);
        assert_eq!(summary.rtt_us, 4_000);
        assert_eq!(summary.loss_ppm, 12);
        assert_eq!(summary.fps, 60);
        assert_eq!(summary.bitrate_bps, 20_000_000);
        assert_eq!(summary.state, SessionState::Closed);
    }

    #[test]
    fn snapshot_never_underflows_on_clock_reset() {
        let counters = SessionCounters {
            started_ms: 10_000,
            ..SessionCounters::default()
        };
        // 换了时钟（时间基准变小）时不能算出负的时长。
        let summary = counters.snapshot(0, SessionState::Idle);
        assert_eq!(summary.duration_ms, 0);
    }
}
