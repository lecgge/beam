//! 解码层错误码与文案映射（§3.10 错误码段 `E10xxx`）。
//!
//! §3.10 把 decode / render 归在同一段 `E10xxx`：
//! `10001` 解码器不可用、`10002` 解码失败、`10003` 设备丢失。
//! `10004` 是本 crate 的**段内扩展**，用来区分「本次构建没带后端」
//! （`ffmpeg` feature 关闭）与「带了后端但本机没有可用硬解」——后者必须报 `E10001`。

/// 解码 / 渲染层错误码编号段 `E10xxx`（§3.10）。
pub mod code {
    /// `E10001`：解码器不可用。
    pub const DECODER_UNAVAILABLE: u16 = 10001;
    /// `E10002`：解码失败。
    pub const DECODE_FAILED: u16 = 10002;
    /// `E10003`：图形设备丢失。
    pub const DEVICE_LOST: u16 = 10003;
    /// `E10004`：后端未启用（§3.10 段内扩展，非文档定义）。
    pub const BACKEND_DISABLED: u16 = 10004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::DECODER_UNAVAILABLE => "没有可用的硬件解码器",
        code::DECODE_FAILED => "画面解码失败",
        code::DEVICE_LOST => "图形设备丢失,需要重建解码器",
        code::BACKEND_DISABLED => "解码后端未启用",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知解码错误";

/// 解码层错误（§3.10 错误码段 `E10xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum DecError {
    /// `E10001`：解码器不可用。
    #[error("E10001: 没有可用的硬件解码器")]
    DecoderUnavailable,

    /// `E10002`：解码失败。
    #[error("E10002: 画面解码失败: {reason}")]
    DecodeFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E10003`：设备丢失。
    #[error("E10003: 图形设备丢失")]
    DeviceLost,

    /// `E10004`：后端未启用。
    #[error("E10004: 解码后端未启用(缺少 ffmpeg feature)")]
    BackendDisabled,
}

impl DecError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            DecError::DecoderUnavailable => code::DECODER_UNAVAILABLE,
            DecError::DecodeFailed { .. } => code::DECODE_FAILED,
            DecError::DeviceLost => code::DEVICE_LOST,
            DecError::BackendDisabled => code::BACKEND_DISABLED,
        }
    }

    /// 返回 `E` 前缀错误码字符串。
    pub fn code_str(&self) -> String {
        format!("E{}", self.code())
    }

    /// 返回人类可读文案。
    pub fn text(&self) -> &'static str {
        message(self.code())
    }

    /// 是否值得让用户重试（§3.11）。
    pub fn retryable(&self) -> bool {
        matches!(self, DecError::DecodeFailed { .. } | DecError::DeviceLost)
    }
}

/// 判断错误码是否属于解码 / 渲染层 `E10xxx` 段。
pub fn is_decode_code(code: u16) -> bool {
    (10000..10100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(DecError::DecoderUnavailable.code(), 10001);
        assert_eq!(DecError::DecodeFailed { reason: "x".into() }.code(), 10002);
        assert_eq!(DecError::DeviceLost.code(), 10003);
        assert_eq!(DecError::BackendDisabled.code(), 10004);
        assert_eq!(message(10099), UNKNOWN_TEXT);
        assert!(is_decode_code(10001));
    }

    #[test]
    fn every_code_has_a_message() {
        for c in [
            code::DECODER_UNAVAILABLE,
            code::DECODE_FAILED,
            code::DEVICE_LOST,
            code::BACKEND_DISABLED,
        ] {
            assert_ne!(message(c), UNKNOWN_TEXT, "错误码 {c} 缺少文案");
        }
    }
}
