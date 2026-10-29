//! 会话层错误码与文案映射（§3.9 错误码段 `E9xxx`）。

/// 会话层错误码编号段 `E9xxx`（§3.9）。
pub mod code {
    /// `E9001`：非法状态转移。
    pub const ILLEGAL_TRANSITION: u16 = 9001;
    /// `E9002`：依赖缺失（例如未注入帧流水线）。
    pub const MISSING_DEPENDENCY: u16 = 9002;
    /// `E9003`：会话超时（重连超时）。
    pub const SESSION_TIMEOUT: u16 = 9003;
    /// `E9004`：会话已关闭。
    pub const SESSION_CLOSED: u16 = 9004;
    /// `E9005`：传输失败。
    pub const TRANSPORT_FAILED: u16 = 9005;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::ILLEGAL_TRANSITION => "会话状态转移非法",
        code::MISSING_DEPENDENCY => "会话依赖缺失",
        code::SESSION_TIMEOUT => "会话超时,已断开",
        code::SESSION_CLOSED => "会话已关闭",
        code::TRANSPORT_FAILED => "网络传输失败",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知会话错误";

/// 会话层错误（§3.9 错误码段 `E9xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// `E9001`：非法状态转移。
    #[error("E9001: 非法状态转移: {from} -> {to}")]
    IllegalTransition {
        /// 源状态。
        from: String,
        /// 目标状态。
        to: String,
    },

    /// `E9002`：依赖缺失。
    #[error("E9002: 会话依赖缺失: {what}")]
    MissingDependency {
        /// 缺失的依赖名。
        what: String,
    },

    /// `E9003`：会话超时。
    #[error("E9003: 会话超时({secs} 秒无响应)")]
    SessionTimeout {
        /// 超时秒数。
        secs: u64,
    },

    /// `E9004`：会话已关闭。
    #[error("E9004: 会话已关闭")]
    SessionClosed,

    /// `E9005`：传输失败。
    #[error("E9005: 网络传输失败: {reason}")]
    TransportFailed {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl SessionError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            SessionError::IllegalTransition { .. } => code::ILLEGAL_TRANSITION,
            SessionError::MissingDependency { .. } => code::MISSING_DEPENDENCY,
            SessionError::SessionTimeout { .. } => code::SESSION_TIMEOUT,
            SessionError::SessionClosed => code::SESSION_CLOSED,
            SessionError::TransportFailed { .. } => code::TRANSPORT_FAILED,
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
            SessionError::SessionTimeout { .. } | SessionError::TransportFailed { .. }
        )
    }
}

/// 判断错误码是否属于会话层 `E9xxx` 段。
pub fn is_session_code(code: u16) -> bool {
    (9000..9100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            SessionError::IllegalTransition {
                from: "a".into(),
                to: "b".into()
            }
            .code(),
            9001
        );
        assert_eq!(
            SessionError::MissingDependency { what: "x".into() }.code(),
            9002
        );
        assert_eq!(SessionError::SessionTimeout { secs: 30 }.code(), 9003);
        assert_eq!(SessionError::SessionClosed.code(), 9004);
        assert_eq!(message(9099), UNKNOWN_TEXT);
        assert!(is_session_code(9001));
    }
}
