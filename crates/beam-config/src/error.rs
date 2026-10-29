//! 配置层错误码与文案映射（§3.14 错误码段 `E11xxx`）。

/// 配置层错误码编号段 `E11xxx`（§3.14）。
pub mod code {
    /// `E11001`：配置文件解析失败。
    pub const PARSE_FAILED: u16 = 11001;
    /// `E11002`：配置校验失败。
    pub const VALIDATION_FAILED: u16 = 11002;
    /// `E11003`：配置存储不可用。
    pub const STORAGE_UNAVAILABLE: u16 = 11003;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::PARSE_FAILED => "配置文件解析失败",
        code::VALIDATION_FAILED => "配置项不合法",
        code::STORAGE_UNAVAILABLE => "配置文件无法读写",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知配置错误";

/// 配置层错误（§3.14 错误码段 `E11xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `E11001`：解析失败。
    #[error("E11001: 配置文件解析失败: {reason}")]
    ParseFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E11002`：校验失败。
    #[error("E11002: 配置项不合法: {reason}")]
    ValidationFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E11003`：存储不可用。
    #[error("E11003: 配置存储不可用: {reason}")]
    StorageUnavailable {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl ConfigError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            ConfigError::ParseFailed { .. } => code::PARSE_FAILED,
            ConfigError::ValidationFailed { .. } => code::VALIDATION_FAILED,
            ConfigError::StorageUnavailable { .. } => code::STORAGE_UNAVAILABLE,
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
        matches!(self, ConfigError::StorageUnavailable { .. })
    }
}

/// 判断错误码是否属于配置层 `E11xxx` 段。
pub fn is_config_code(code: u16) -> bool {
    (11000..12000).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            ConfigError::ParseFailed { reason: "x".into() }.code(),
            11001
        );
        assert_eq!(
            ConfigError::ValidationFailed { reason: "x".into() }.code(),
            11002
        );
        assert_eq!(
            ConfigError::StorageUnavailable { reason: "x".into() }.code(),
            11003
        );
        assert_eq!(message(11999), UNKNOWN_TEXT);
        assert!(is_config_code(11001));
    }
}
