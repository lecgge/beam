//! 配对层错误码与文案映射（§3.3 错误码段 `E3xxx`）。
//!
//! §6 约定「各 crate 定义 thiserror 枚举并映射到 `E` 码」，
//! §3.11 约定「UI 只根据 code 显示文案」，因此码与文案必须能互相反查。

/// 配对层错误码编号段 `E3xxx`（§3.3）。
pub mod code {
    /// `E3001`：PIN 错误。
    pub const PIN_INCORRECT: u16 = 3001;
    /// `E3002`：尝试次数超限，锁定 60 s。
    pub const ATTEMPTS_EXCEEDED: u16 = 3002;
    /// `E3003`：token 过期。
    pub const TOKEN_EXPIRED: u16 = 3003;
    /// `E3004`：指纹不匹配。
    pub const FINGERPRINT_MISMATCH: u16 = 3004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::PIN_INCORRECT => "PIN 码错误",
        code::ATTEMPTS_EXCEEDED => "尝试次数过多,已临时锁定",
        code::TOKEN_EXPIRED => "配对码已过期,请重新扫码",
        code::FINGERPRINT_MISMATCH => "设备指纹不匹配",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知配对错误";

/// 配对层错误（§3.3 错误码段 `E3xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum PairError {
    /// `E3001`：SPAKE2 交换失败，通常是 PIN 不一致。
    #[error("E3001: PIN 码错误")]
    PinIncorrect,

    /// `E3002`：连续失败次数超限，已锁定。
    #[error("E3002: 尝试次数超过上限 {max_attempts},已锁定 {lockout_secs} 秒")]
    AttemptsExceeded {
        /// 允许的最大尝试次数。
        max_attempts: u32,
        /// 锁定时长，秒。
        lockout_secs: u64,
    },

    /// `E3003`：一次性 token 超过有效期。
    #[error("E3003: 配对码已过期")]
    TokenExpired,

    /// `E3004`：对端指纹与信任库不符。
    #[error("E3004: 设备指纹不匹配")]
    FingerprintMismatch,

    /// `E3005`：SPAKE2 消息格式非法（§3.3 段内扩展：协议层无法解析挑战）。
    #[error("E3005: SPAKE2 消息非法")]
    MalformedChallenge,

    /// `E3006`：身份材料读写失败（§3.3 段内扩展：证书 / 私钥持久化）。
    #[error("E3006: 身份材料读写失败: {reason}")]
    Storage {
        /// 底层原因，仅用于日志。
        reason: String,
    },

    /// `E3007`：二维码载荷格式非法（§3.3 段内扩展）。
    #[error("E3007: 配对二维码内容非法: {reason}")]
    InvalidPayload {
        /// 底层原因，仅用于日志。
        reason: String,
    },
}

impl PairError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            PairError::PinIncorrect => code::PIN_INCORRECT,
            PairError::AttemptsExceeded { .. } => code::ATTEMPTS_EXCEEDED,
            PairError::TokenExpired => code::TOKEN_EXPIRED,
            PairError::FingerprintMismatch => code::FINGERPRINT_MISMATCH,
            PairError::MalformedChallenge => 3005,
            PairError::Storage { .. } => 3006,
            PairError::InvalidPayload { .. } => 3007,
        }
    }

    /// 返回 `E` 前缀错误码字符串，例如 `"E3001"`。
    pub fn code_str(&self) -> String {
        format!("E{}", self.code())
    }

    /// 返回人类可读文案。
    pub fn text(&self) -> &'static str {
        message(self.code())
    }

    /// 是否值得让用户重试（§3.11 的 `retryable` 字段）。
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            PairError::PinIncorrect | PairError::TokenExpired | PairError::AttemptsExceeded { .. }
        )
    }
}

/// 判断错误码是否属于配对层 `E3xxx` 段。
pub fn is_pair_code(code: u16) -> bool {
    (3000..3100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_and_mapped() {
        let errs = [
            PairError::PinIncorrect,
            PairError::AttemptsExceeded {
                max_attempts: 3,
                lockout_secs: 60,
            },
            PairError::TokenExpired,
            PairError::FingerprintMismatch,
            PairError::MalformedChallenge,
            PairError::Storage { reason: "x".into() },
            PairError::InvalidPayload { reason: "x".into() },
        ];
        let mut codes: Vec<u16> = errs.iter().map(|e| e.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errs.len(), "错误码必须互不相同");
        assert_eq!(PairError::PinIncorrect.code(), 3001);
        assert_eq!(
            PairError::AttemptsExceeded {
                max_attempts: 3,
                lockout_secs: 60
            }
            .code(),
            3002
        );
        assert_eq!(PairError::TokenExpired.code(), 3003);
        assert_eq!(PairError::FingerprintMismatch.code(), 3004);
    }

    #[test]
    fn unknown_code_has_fallback() {
        assert_eq!(message(3999), UNKNOWN_TEXT);
        assert!(is_pair_code(3001));
        assert!(!is_pair_code(2001));
    }
}
