//! 日志层错误（§3.15 使用 `E12xxx` 段，§3.1 只规定到 `E11xxx`，此处顺延）。
//!
//! §6 要求「各 crate 定义 thiserror 枚举并映射到 `E` 码」，
//! 因此日志层也给出稳定的错误码。

/// 日志层错误码编号段 `E12xxx`。
pub mod code {
    /// `E12001`：日志初始化失败。
    pub const INIT_FAILED: u16 = 12001;
}

/// 由错误码反查人类可读文案。
pub fn message(code: u16) -> &'static str {
    match code {
        code::INIT_FAILED => "日志初始化失败",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知日志错误";

/// 日志层错误。
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// `E12001`：初始化失败。
    #[error("E12001: 日志初始化失败: {reason}")]
    Init {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl LogError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            LogError::Init { .. } => code::INIT_FAILED,
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

    /// 是否值得让用户重试。
    pub fn retryable(&self) -> bool {
        false
    }
}

/// 判断错误码是否属于日志层 `E12xxx` 段。
pub fn is_log_code(code: u16) -> bool {
    (12000..13000).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_stable() {
        let err = LogError::Init { reason: "x".into() };
        assert_eq!(err.code(), 12001);
        assert_eq!(err.code_str(), "E12001");
        assert_eq!(err.text(), message(12001));
        assert!(is_log_code(12001));
    }
}
