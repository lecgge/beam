//! 性能采集阶段（§3.13）。
//!
//! §3.13 列举的阶段：抓屏 / 颜色转换 / 编码 / 分包 / RTT / 丢包 /
//! FEC 恢复 / 抖动缓冲 / 解码 / 渲染 / 端到端。
//!
//! ## 变体序号策略（线上兼容性）
//!
//! `Stage` 的**变体序号即线上编码**：快照按序号排序后导出（CSV 行序 /
//! JSON 数组序），跨版本比对时也以序号为准。因此：
//!
//! - **既有变体的序号不得调整**。插入新变体只能追加到末尾。
//! - 端到端阶段 `Stage::E2E` 追加在 §3.13 所列阶段的**末尾**
//!   （`Render` 之后），`Stage::Other` 作为兜底变体始终是最后一个。
//!   这样 `Capture..=Render` 这 10 个既有阶段的序号保持不变，
//!   `Other` 的序号从 10 变为 11。
//! - 线上兼容性影响：`Other` 的序号变化只影响「兜底阶段」这一项的编码，
//!   其余 10 个阶段的线上编码不变；由于 `Other` 不对应任何真实阶段，
//!   实际线上数据不受影响。新增阶段一律追加，不得在中间插入。

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

    /// 线上编码：变体序号。
    ///
    /// 见模块文档「变体序号策略」：序号一旦发布就不得调整，
    /// 新增阶段只能追加到末尾。
    pub const fn ordinal(self) -> u8 {
        self as u8
    }

    /// 由线上编码（变体序号）还原阶段。
    ///
    /// 未知编码返回 `None`（不静默映射到 `Other`，避免把未知阶段
    /// 混进兜底阶段而污染统计）。
    pub const fn from_ordinal(code: u8) -> Option<Stage> {
        match code {
            0 => Some(Stage::Capture),
            1 => Some(Stage::ColorConvert),
            2 => Some(Stage::Encode),
            3 => Some(Stage::Packetize),
            4 => Some(Stage::Rtt),
            5 => Some(Stage::Loss),
            6 => Some(Stage::FecRecover),
            7 => Some(Stage::Jitter),
            8 => Some(Stage::Decode),
            9 => Some(Stage::Render),
            10 => Some(Stage::E2E),
            11 => Some(Stage::Other),
            _ => None,
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

    #[test]
    fn e2e_stage_exists() {
        // §3.13 的阶段列表包含「端到端」。
        assert!(ALL_STAGES.contains(&Stage::E2E));
        assert_eq!(Stage::from_name("e2e"), Stage::E2E);
        assert_eq!(Stage::E2E.as_str(), "e2e");
    }

    #[test]
    fn ordinals_are_stable() {
        // 变体序号即线上编码：既有阶段序号不得调整，新阶段只能追加到末尾。
        assert_eq!(Stage::Capture.ordinal(), 0);
        assert_eq!(Stage::ColorConvert.ordinal(), 1);
        assert_eq!(Stage::Encode.ordinal(), 2);
        assert_eq!(Stage::Packetize.ordinal(), 3);
        assert_eq!(Stage::Rtt.ordinal(), 4);
        assert_eq!(Stage::Loss.ordinal(), 5);
        assert_eq!(Stage::FecRecover.ordinal(), 6);
        assert_eq!(Stage::Jitter.ordinal(), 7);
        assert_eq!(Stage::Decode.ordinal(), 8);
        assert_eq!(Stage::Render.ordinal(), 9);
        assert_eq!(Stage::E2E.ordinal(), 10);
        assert_eq!(Stage::Other.ordinal(), 11);
    }

    #[test]
    fn ordinal_round_trips() {
        for s in ALL_STAGES {
            assert_eq!(Stage::from_ordinal(s.ordinal()), Some(s));
        }
        assert_eq!(Stage::from_ordinal(12), None);
    }
}
