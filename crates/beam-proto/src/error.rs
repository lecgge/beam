//! 协议层错误码与文案映射（§3.1、§3.11、§6）。
//!
//! §6 约定「各 crate 定义 thiserror 枚举并映射到 `E` 码」，
//! §3.11 约定「command 返回 `{code:"E2001", message, retryable}`，前端按 code 显示中文文案」。
//! 因此每个错误变体都必须能反查出一个稳定的数字码，码再反查出一句人类可读文案。

/// 协议层错误码编号段 `E1xxx`（§3.1）。
pub mod code {
    /// `E1001`：协议版本不兼容（§5 版本协商：主版本不同直接拒绝）。
    pub const VERSION_INCOMPATIBLE: u16 = 1001;
    /// `E1002`：消息解码失败（postcard 反序列化失败）。
    pub const DECODE_FAILED: u16 = 1002;
    /// `E1003`：未知消息类型（枚举变体序号不在已知集合内）。
    pub const UNKNOWN_MESSAGE: u16 = 1003;
    /// `E1004`：能力协商失败（无公共编解码器 / 分辨率 / 帧率交集）。
    ///
    /// §3.1 只枚举了 1001–1003 三种；本条落在同一 `E1xxx` 段内，
    /// 用于承载「双方 Hello 的能力集无交集」这一真实且无法归入前三者的失败。
    pub const CAPABILITY_MISMATCH: u16 = 1004;
}

/// 由错误码反查人类可读文案（§3.11：UI 只根据 code 显示文案）。
///
/// 未知码返回 [`UNKNOWN_MESSAGE_TEXT`]，保证调用方永远拿得到可展示的字符串。
pub fn message(code: u16) -> &'static str {
    match code {
        code::VERSION_INCOMPATIBLE => "协议版本不兼容",
        code::DECODE_FAILED => "消息解码失败",
        code::UNKNOWN_MESSAGE => "未知消息类型",
        code::CAPABILITY_MISMATCH => "双方能力无交集,协商失败",
        _ => UNKNOWN_MESSAGE_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_MESSAGE_TEXT: &str = "未知协议错误";

/// 协议层错误（§3.1 错误码段 `E1xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// `E1001`：Hello 握手时主版本不同，按 §5 规则直接拒绝。
    #[error("E1001: 协议版本不兼容(本地主版本 {local},远端主版本 {remote})")]
    VersionIncompatible {
        /// 本地主版本号。
        local: u8,
        /// 远端主版本号。
        remote: u8,
    },

    /// `E1002`：postcard 反序列化失败（含帧头字节不足等二进制解析失败）。
    #[error("E1002: 消息解码失败: {reason}")]
    DecodeFailed {
        /// 底层失败原因，仅用于日志，不得直接展示给用户。
        reason: String,
    },

    /// `E1003`：消息变体序号超出已知集合。
    #[error("E1003: 未知消息类型(变体序号 {variant})")]
    UnknownMessage {
        /// 读到的变体序号。
        variant: u32,
    },

    /// `E1004`：能力协商没有交集。
    #[error("E1004: 能力协商失败: {reason}")]
    CapabilityMismatch {
        /// 协商失败原因，仅用于日志。
        reason: String,
    },
}

impl ProtoError {
    /// 返回该错误的数字错误码（§3.11 的 `code` 字段去掉 `E` 前缀）。
    pub fn code(&self) -> u16 {
        match self {
            ProtoError::VersionIncompatible { .. } => code::VERSION_INCOMPATIBLE,
            ProtoError::DecodeFailed { .. } => code::DECODE_FAILED,
            ProtoError::UnknownMessage { .. } => code::UNKNOWN_MESSAGE,
            ProtoError::CapabilityMismatch { .. } => code::CAPABILITY_MISMATCH,
        }
    }

    /// 返回 `E` 前缀的字符串错误码，例如 `"E1001"`（§3.11 的 `code` 字段格式）。
    pub fn code_str(&self) -> String {
        format!("E{}", self.code())
    }

    /// 返回人类可读文案（§3.11 的 `message` 字段）。
    pub fn text(&self) -> &'static str {
        message(self.code())
    }

    /// 该错误是否值得让用户重试（§3.11 的 `retryable` 字段）。
    pub fn retryable(&self) -> bool {
        match self {
            ProtoError::VersionIncompatible { .. } => false,
            ProtoError::DecodeFailed { .. } => false,
            ProtoError::UnknownMessage { .. } => false,
            ProtoError::CapabilityMismatch { .. } => false,
        }
    }
}

/// 由错误码反查该码是否属于协议层 `E1xxx` 段。
pub fn is_proto_code(code: u16) -> bool {
    (1000..1100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_and_text_round_trip() {
        let err = ProtoError::VersionIncompatible {
            local: 1,
            remote: 2,
        };
        assert_eq!(err.code(), 1001);
        assert_eq!(err.code_str(), "E1001");
        assert_eq!(err.text(), message(1001));
        assert!(is_proto_code(err.code()));
    }

    #[test]
    fn every_variant_has_distinct_code() {
        let codes = [
            ProtoError::VersionIncompatible {
                local: 1,
                remote: 2,
            }
            .code(),
            ProtoError::DecodeFailed { reason: "x".into() }.code(),
            ProtoError::UnknownMessage { variant: 9 }.code(),
            ProtoError::CapabilityMismatch { reason: "x".into() }.code(),
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "错误码必须互不相同");
    }

    #[test]
    fn unknown_code_has_fallback_text() {
        assert_eq!(message(9999), UNKNOWN_MESSAGE_TEXT);
    }
}
