//! FFI 层错误码与文案映射。
//!
//! §3.12：Android 侧按 `code` 显示文案；这里沿用 §6 的 `E` 码约定，
//! 使用 `E14xxx` 段（`E11xxx` 已被配置层占用，`E12xxx` 被日志层占用）。

// crate 级 lint 在 `uniffi` feature 打开时放宽为 `allow`（uniffi scaffolding 含 unsafe impl），
// 这里用模块级 `deny` 把业务代码重新收紧。
#![deny(unsafe_code)]

/// FFI 层错误码编号段 `E14xxx`。
pub mod code {
    /// `E14001`：配对载荷非法。
    pub const INVALID_PAYLOAD: u16 = 14001;
    /// `E14002`：PIN 配对失败。
    pub const PIN_FAILED: u16 = 14002;
    /// `E14003`：连接失败。
    pub const CONNECT_FAILED: u16 = 14003;
    /// `E14004`：发现服务不可用。
    pub const DISCOVERY_FAILED: u16 = 14004;
    /// `E14005`：功能未启用。
    pub const UNSUPPORTED: u16 = 14005;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::INVALID_PAYLOAD => "配对二维码内容无效",
        code::PIN_FAILED => "PIN 码校验失败",
        code::CONNECT_FAILED => "连接主机失败",
        code::DISCOVERY_FAILED => "无法发现局域网主机",
        code::UNSUPPORTED => "当前平台不支持该功能",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知错误";

/// FFI 层错误。
#[derive(Debug, thiserror::Error)]
pub enum FfiError {
    /// `E14001`：配对载荷非法。
    #[error("E14001: 配对二维码内容无效: {reason}")]
    InvalidPayload {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E14002`：PIN 配对失败。
    #[error("E14002: PIN 码校验失败")]
    PinFailed,

    /// `E14003`：连接失败。
    #[error("E14003: 连接主机失败: {reason}")]
    ConnectFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E14004`：发现服务不可用。
    #[error("E14004: 无法发现局域网主机")]
    DiscoveryFailed,

    /// `E14005`：功能未启用。
    #[error("E14005: 当前平台不支持该功能")]
    Unsupported,
}

impl FfiError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            FfiError::InvalidPayload { .. } => code::INVALID_PAYLOAD,
            FfiError::PinFailed => code::PIN_FAILED,
            FfiError::ConnectFailed { .. } => code::CONNECT_FAILED,
            FfiError::DiscoveryFailed => code::DISCOVERY_FAILED,
            FfiError::Unsupported => code::UNSUPPORTED,
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
            FfiError::PinFailed | FfiError::ConnectFailed { .. } | FfiError::DiscoveryFailed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            FfiError::InvalidPayload { reason: "x".into() }.code(),
            14001
        );
        assert_eq!(FfiError::PinFailed.code(), 14002);
        assert_eq!(FfiError::ConnectFailed { reason: "x".into() }.code(), 14003);
        assert_eq!(FfiError::DiscoveryFailed.code(), 14004);
        assert_eq!(FfiError::Unsupported.code(), 14005);
        assert_eq!(message(14999), UNKNOWN_TEXT);
    }
}
