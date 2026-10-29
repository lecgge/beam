//! 编码层错误码与文案映射（§3.6 错误码段 `E6xxx`）。

/// 编码层错误码编号段 `E6xxx`（§3.6）。
pub mod code {
    /// `E6001`：无可用硬编。
    pub const NO_HARDWARE_ENCODER: u16 = 6001;
    /// `E6002`：编码会话数超限。
    pub const SESSION_LIMIT: u16 = 6002;
    /// `E6003`：参数不支持，应降档。
    pub const UNSUPPORTED_PARAM: u16 = 6003;
    /// `E6004`：编码后端未启用（§3.6 段内扩展）。
    pub const BACKEND_DISABLED: u16 = 6004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::NO_HARDWARE_ENCODER => "没有可用的硬件编码器",
        code::SESSION_LIMIT => "硬件编码会话数已达上限",
        code::UNSUPPORTED_PARAM => "编码参数不被支持,已降档",
        code::BACKEND_DISABLED => "编码后端未启用",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知编码错误";

/// 编码层错误（§3.6 错误码段 `E6xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum EncError {
    /// `E6001`：无可用硬编。
    #[error("E6001: 没有可用的硬件编码器")]
    NoHardwareEncoder,

    /// `E6002`：会话数超限。
    #[error("E6002: 硬件编码会话数已达上限")]
    SessionLimit,

    /// `E6003`：参数不支持。
    #[error("E6003: 编码参数不被支持: {what}")]
    UnsupportedParam {
        /// 不被支持的参数，仅用于日志。
        what: String,
    },

    /// `E6004`：后端未启用。
    #[error("E6004: 编码后端未启用(缺少 ffmpeg feature)")]
    BackendDisabled,
}

impl EncError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            EncError::NoHardwareEncoder => code::NO_HARDWARE_ENCODER,
            EncError::SessionLimit => code::SESSION_LIMIT,
            EncError::UnsupportedParam { .. } => code::UNSUPPORTED_PARAM,
            EncError::BackendDisabled => code::BACKEND_DISABLED,
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
        matches!(
            self,
            EncError::SessionLimit | EncError::UnsupportedParam { .. }
        )
    }
}

/// 判断错误码是否属于编码层 `E6xxx` 段。
pub fn is_encode_code(code: u16) -> bool {
    (6000..6100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_codes_are_stable() {
        assert_eq!(EncError::NoHardwareEncoder.code(), 6001);
        assert_eq!(EncError::SessionLimit.code(), 6002);
        assert_eq!(EncError::UnsupportedParam { what: "x".into() }.code(), 6003);
        assert_eq!(EncError::BackendDisabled.code(), 6004);
        assert_eq!(message(6099), UNKNOWN_TEXT);
        assert!(is_encode_code(6001));
    }
}
