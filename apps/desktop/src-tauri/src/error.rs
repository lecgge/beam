//! 统一错误模型。
//!
//! §3.11 规定:command 失败时返回 `{code:"E2001", message, retryable}`,
//! 前端只按 `code` 显示中文文案(文案表见前端 `src/ipc/errors.ts`)。
//!
//! 错误码沿用 §3.1–§3.15 的分段约定:
//! `E1xxx` proto / `E2xxx` net / `E3xxx` pair / `E4xxx` discovery / `E5xxx` capture /
//! `E6xxx` encode / `E7xxx` audio / `E8xxx` input / `E9xxx` session /
//! `E10xxx` decode-render / `E11xxx` config / `E12xxx` 应用外壳。

use serde::Serialize;

// ---- E2xxx net(§3.2)----
pub const E_CONNECTION_TIMEOUT: &str = "E2001";
pub const E_CERT_REJECTED: &str = "E2002";
pub const E_DATAGRAM_UNSUPPORTED: &str = "E2003";
pub const E_FRAME_EXPIRED: &str = "E2004";

// ---- E3xxx pair(§3.3)----
pub const E_PIN_INVALID: &str = "E3001";
pub const E_PIN_LOCKED: &str = "E3002";
pub const E_TOKEN_EXPIRED: &str = "E3003";
pub const E_FINGERPRINT_MISMATCH: &str = "E3004";

// ---- E4xxx discovery(§3.4)----
pub const E_DISCOVERY_FAILED: &str = "E4001";

// ---- E5xxx capture(§3.5)----
pub const E_NO_DISPLAY: &str = "E5001";
pub const E_EXCLUSIVE_FULLSCREEN: &str = "E5002";
pub const E_DEVICE_LOST: &str = "E5003";

// ---- E6xxx encode(§3.6)----
pub const E_NO_ENCODER: &str = "E6001";
pub const E_SESSION_LIMIT: &str = "E6002";
pub const E_PARAM_UNSUPPORTED: &str = "E6003";

// ---- E9xxx session(§3.9)----
pub const E_SESSION_INVALID_STATE: &str = "E9001";
pub const E_SESSION_ALREADY_ACTIVE: &str = "E9002";
pub const E_RECONNECT_FAILED: &str = "E9003";
pub const E_DEVICE_NOT_FOUND: &str = "E9101";
pub const E_HOST_NOT_FOUND: &str = "E9102";

// ---- E11xxx config(§3.14)----
pub const E_CONFIG_INVALID: &str = "E11001";
pub const E_CONFIG_IO: &str = "E11002";
/// perf-stats 仅 Debug 构建可用(§3.13 门控 / §7 release 无 perf 符号)。
pub const E_PERF_UNAVAILABLE: &str = "E11003";

// ---- E12xxx 应用外壳(§3.11 host-app)----
pub const E_UNKNOWN_COMMAND: &str = "E12001";
pub const E_INTERNAL: &str = "E9999";

/// §3.11 约定的 command 错误负载。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeamError {
    /// 形如 `"E2001"` 的稳定错误码,前端据此查中文文案。
    pub code: String,
    /// 英文/技术性说明,供日志与调试;面向用户的文案以 code 查表为准。
    pub message: String,
    /// 是否建议用户重试(网络类为 true,参数/权限类为 false)。
    pub retryable: bool,
}

impl BeamError {
    pub fn new(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable,
        }
    }

    /// 参数/配置类错误:不可重试。
    pub fn invalid(code: &str, message: impl Into<String>) -> Self {
        Self::new(code, message, false)
    }

    /// 网络/时序类错误:可重试。
    pub fn retryable(code: &str, message: impl Into<String>) -> Self {
        Self::new(code, message, true)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(E_INTERNAL, message, false)
    }
}

impl std::fmt::Display for BeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for BeamError {}
