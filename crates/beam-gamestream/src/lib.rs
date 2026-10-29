//! # beam-gamestream —— Beam GameStream 兼容层（v1）
//!
//! 对应技术设计文档 **§2**（仓库结构）与 **§5**（兼容策略）。
//!
//! ## 净室声明（§5 / §7 / §14）
//!
//! 本 crate **只按公开协议行为实现**，不复制、不翻译、不改写任何
//! Sunshine / Moonlight 的 GPL 代码，也不参考其源码。
//! 实现依据是公开可观察的协议行为（握手报文结构、编解码协商结果、
//! 输入事件语义等），与本仓库其余 crate 的净室原则一致。
//!
//! ## 职责（v1 预留骨架）
//!
//! - 描述 GameStream 侧的握手 / 能力协商差异；
//! - 把 GameStream 的能力集映射到本仓库的 [`beam_proto::Caps`]；
//! - 明确「哪些行为已实现、哪些属于 M3」。
//!
//! ## 里程碑
//!
//! M3（v1）。本轮只交付**最小骨架**：能力映射表 + 特性开关 + 占位错误码，
//! 不含任何真实握手报文。
//!
//! ## feature
//!
//! `gamestream`（§7，v1 开启）。

#![forbid(unsafe_code)]

pub mod capability;

pub use capability::{
    GameStreamCaps, GameStreamCompat, GAMESTREAM_SERVICE_TYPE, GAMESTREAM_VERSION,
};

/// 兼容层错误码段 `E15xxx`。
pub mod code {
    /// `E15001`：协议版本不支持。
    pub const VERSION_UNSUPPORTED: u16 = 15001;
    /// `E15002`：能力不兼容。
    pub const CAPABILITY_INCOMPATIBLE: u16 = 15002;
}

/// 由错误码反查人类可读文案。
pub fn message(code: u16) -> &'static str {
    match code {
        code::VERSION_UNSUPPORTED => "GameStream 协议版本不兼容",
        code::CAPABILITY_INCOMPATIBLE => "与 GameStream 主机的能力无交集",
        _ => "未知 GameStream 兼容错误",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            message(code::VERSION_UNSUPPORTED),
            "GameStream 协议版本不兼容"
        );
        assert_eq!(
            message(code::CAPABILITY_INCOMPATIBLE),
            "与 GameStream 主机的能力无交集"
        );
        assert!(message(15999).contains("未知"));
    }
}
