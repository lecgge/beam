//! 统计上报（§3.1 `StatsReport`、§3.11 `session://stats` 500 ms 节拍）。
//!
//! 全部使用整数微秒 / 百万分比，便于 postcard 序列化（postcard 不支持浮点）。

use serde::{Deserialize, Serialize};

/// 客户端回报的串流统计（§5 控制消息 `StatsReport`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsReport {
    /// 会话号（`Resume` 复用）。
    pub session_id: u64,
    /// 实际接收帧率。
    pub fps: u32,
    /// 实测接收码率，bit/s。
    pub bitrate_bps: u32,
    /// 丢包率，百万分比（0..=1_000_000）。
    pub loss_ppm: u32,
    /// 往返时延，微秒。
    pub rtt_us: u32,
    /// 解码耗时，微秒。
    pub decode_us: u32,
    /// 上屏等待，微秒。
    pub render_us: u32,
    /// 端到端延迟，微秒（§8 验收口径）。
    pub e2e_us: u32,
    /// 累计掉帧数。
    pub dropped_frames: u32,
}

impl Default for StatsReport {
    fn default() -> Self {
        Self {
            session_id: 0,
            fps: 0,
            bitrate_bps: 0,
            loss_ppm: 0,
            rtt_us: 0,
            decode_us: 0,
            render_us: 0,
            e2e_us: 0,
            dropped_frames: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zeroed() {
        let s = StatsReport::default();
        assert_eq!(s.fps, 0);
        assert_eq!(s.e2e_us, 0);
    }
}
