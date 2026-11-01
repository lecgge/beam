//! 日志层错误（§3.15 使用 `E16xxx` 段）。
//!
//! §6 要求「各 crate 定义 thiserror 枚举并映射到 `E` 码」，
//! 因此日志层也给出稳定的错误码。
//!
//! ## 为什么是 `E16xxx` 而不是 `E12xxx`
//!
//! 早先这里占的是 `E12xxx`（`E12001` 日志初始化失败），而**同一段位**
//! 已被桌面端应用外壳拿去当「未知命令」用（§3.11）—— 同一个码两种含义，
//! 前端按 `code` 查文案时无法定位到底是哪一边失败。
//!
//! S5 裁决把 `E12xxx` 整段划给**桌面端应用外壳**，`E11xxx` 划给
//! `beam-config`（§3.14），日志层因此迁到 `E16xxx`（§3.15）——
//! 与 `E13xxx` perf / `E14xxx` ffi / `E15xxx` gamestream 并列，
//! **每个错误码只有唯一归属**。

/// 日志层错误码编号段 `E16xxx`（§3.15）。
pub mod code {
    /// `E16001`：日志初始化失败。
    pub const INIT_FAILED: u16 = 16001;
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
    /// `E16001`：初始化失败。
    #[error("E16001: 日志初始化失败: {reason}")]
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

/// 判断错误码是否属于日志层 `E16xxx` 段。
pub fn is_log_code(code: u16) -> bool {
    (16000..17000).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_stable() {
        let err = LogError::Init { reason: "x".into() };
        assert_eq!(err.code(), 16001);
        assert_eq!(err.code_str(), "E16001");
        assert_eq!(err.text(), message(16001));
        assert!(is_log_code(16001));
    }

    /// 日志层与桌面外壳**不能**再共用 `E12001`：
    /// 同一个码两种含义会让前端按 `code` 查文案时无法定位。
    #[test]
    fn log_code_does_not_collide_with_shell_segment() {
        assert!(!is_log_code(12001));
        assert!(!is_log_code(11001));
    }
}
