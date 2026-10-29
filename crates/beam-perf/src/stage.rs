//! 性能采集阶段（§3.13）。
//!
//! §3.13 列举的阶段：抓屏 / 颜色转换 / 编码 / 分包 / RTT / 丢包 /
//! FEC 恢复 / 抖动缓冲 / 解码 / 渲染 / 端到端。

/// 采集阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// 抓屏。
    Capture,
    /// 颜色转换。
    ColorConvert,
    /// 编码。
    Encode,
    /// 分包 + FEC 编码。
    Packetize,
    /// 往返时延。
    Rtt,
    /// 丢包。
    Loss,
    /// FEC 恢复。
    FecRecover,
    /// 抖动缓冲。
    Jitter,
    /// 解码。
    Decode,
    /// 上屏。
    Render,
    /// 端到端。
    E2E,
    /// 其他。
    Other,
}

/// 全部阶段（用于遍历与导出）。
pub const ALL_STAGES: [Stage; 11] = [
    Stage::Capture,
    Stage::ColorConvert,
    Stage::Encode,
    Stage::Packetize,
    Stage::Rtt,
    Stage::Loss,
    Stage::FecRecover,
    Stage::Jitter,
    Stage::Decode,
    Stage::Render,
    Stage::E2E,
];

impl Stage {
    /// 由名字解析（供 `perf_span!("encode")` 使用）。
    pub fn from_name(name: &str) -> Stage {
        match name {
            "capture" => Stage::Capture,
            "color_convert" => Stage::ColorConvert,
            "encode" => Stage::Encode,
            "packetize" => Stage::Packetize,
            "rtt" => Stage::Rtt,
            "loss" => Stage::Loss,
            "fec_recover" => Stage::FecRecover,
            "jitter" => Stage::Jitter,
            "decode" => Stage::Decode,
            "render" => Stage::Render,
            "e2e" => Stage::E2E,
            _ => Stage::Other,
        }
    }

    /// 稳定的英文名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::Capture => "capture",
            Stage::ColorConvert => "color_convert",
            Stage::Encode => "encode",
            Stage::Packetize => "packetize",
            Stage::Rtt => "rtt",
            Stage::Loss => "loss",
            Stage::FecRecover => "fec_recover",
            Stage::Jitter => "jitter",
            Stage::Decode => "decode",
            Stage::Render => "render",
            Stage::E2E => "e2e",
            Stage::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in ALL_STAGES {
            assert_eq!(Stage::from_name(s.as_str()), s);
        }
    }

    #[test]
    fn unknown_name_falls_back_to_other() {
        assert_eq!(Stage::from_name("nope"), Stage::Other);
    }
}
