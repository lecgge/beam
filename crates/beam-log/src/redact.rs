//! 敏感信息脱敏（§3.15「禁止记录 PIN、私钥、token」）。
//!
//! 这一层提供两类工具：
//! - [`Sensitive`]：包裹敏感值，`Debug` / `Display` 只输出掩码，杜绝误打日志；
//! - 若干 `redact_*` 函数：给需要拼接字符串的场景使用。

use std::fmt;

/// 脱敏后的占位文本。
pub const REDACTED: &str = "***";

/// 敏感值包装：任何日志输出都只会看到掩码。
#[derive(Clone, PartialEq, Eq)]
pub struct Sensitive<T> {
    value: T,
}

impl<T> Sensitive<T> {
    /// 包装敏感值。
    pub fn new(value: T) -> Self {
        Self { value }
    }

    /// 取回原始值（调用方需要显式要求，避免随手打印）。
    pub fn expose(&self) -> &T {
        &self.value
    }

    /// 消费并取回原始值。
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> fmt::Display for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// 脱敏 PIN：只保留长度。
pub fn redact_pin(pin: &str) -> String {
    format!("{REDACTED}(len={})", pin.chars().count())
}

/// 脱敏一次性 token：只保留长度。
pub fn redact_token(token: &str) -> String {
    format!("{REDACTED}(len={})", token.chars().count())
}

/// 脱敏私钥 / 密钥材料：只保留字节数。
pub fn redact_secret(bytes: &[u8]) -> String {
    format!("{REDACTED}(len={})", bytes.len())
}

/// 脱敏证书指纹：保留前 8 位十六进制（§3.4 TXT 里也是前 8 位）。
pub fn redact_fingerprint(hex: &str) -> String {
    if hex.len() <= 8 {
        return REDACTED.to_string();
    }
    format!("{}…", &hex[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_never_prints_value() {
        let s = Sensitive::new("hunter2");
        assert_eq!(format!("{s:?}"), REDACTED);
        assert_eq!(format!("{s}"), REDACTED);
        assert_eq!(s.expose(), &"hunter2");
    }

    #[test]
    fn redact_helpers_hide_content() {
        assert!(!redact_pin("1234").contains("1234"));
        assert!(!redact_token("abcdef").contains("abcdef"));
        assert!(!redact_secret(b"\x01\x02").contains("01"));
        assert_eq!(redact_fingerprint("0123456789abcdef"), "01234567…");
    }
}
