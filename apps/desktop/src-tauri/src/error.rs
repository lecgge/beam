//! 统一错误模型。
//!
//! §3.11 规定:command 失败时返回 `{code:"E2001", message, retryable}`,
//! 前端只按 `code` 显示中文文案(文案表见前端 `src/ipc/errors.ts`)。
//!
//! 错误码沿用 §3.1–§3.15 的分段约定:
//! `E1xxx` proto / `E2xxx` net / `E3xxx` pair / `E4xxx` discovery / `E5xxx` capture /
//! `E6xxx` encode / `E7xxx` audio / `E8xxx` input / `E9xxx` session /
//! `E10xxx` decode-render / `E11xxx` config / `E12xxx` 应用外壳 /
//! `E13xxx` perf / `E14xxx` ffi / `E15xxx` gamestream / `E16xxx` log。
//!
//! ## 为什么外壳段是 `E12xxx`(S5 裁决)
//!
//! 早先外壳把「设置不合法 / 读写失败 / perf 不可用」也塞在 `E11xxx`,
//! 而 `E11xxx` 是 **`beam-config` 独占**的段(§3.14)——
//! `E11003` 一个码两种含义(perf 不可用 vs 配置存储不可用),
//! 前端按 `code` 只能写合并文案,报错不可定位。
//!
//! 因此:`E11xxx` **只**归 `beam-config`;外壳自有错误全部改用 `E12xxx`,
//! 按语义细分(未知命令 / 设置不合法 / 读写失败 / perf 不可用),
//! **一个码只对应一种含义**。

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

// ---- E10xxx decode-render(§3.10)----
/// 客户端上屏失败（原生窗口建不出来 / 交换链创建失败 / 硬解后端不可用）。
///
/// 与 `beam-render` 的 `E10012`（交换链创建失败）对齐。
pub const E_PRESENT_FAILED: &str = "E10012";

// ---- E12xxx 应用外壳(§3.11 host-app)----
//
// `E11xxx` 归 `beam-config` 独占(§3.14),外壳自有错误全部在这里(S5 裁决)。
/// 未知命令。
pub const E_UNKNOWN_COMMAND: &str = "E12001";
/// 设置项取值不合法（外壳层自己的校验，如分辨率/帧率/编码器白名单）。
pub const E_SHELL_CONFIG_INVALID: &str = "E12002";
/// 外壳自己的文件读写失败（桌面设置 JSON、信任库、导出文件）。
pub const E_SHELL_IO: &str = "E12003";
/// perf-stats 仅 Debug 构建可用(§3.13 门控 / §7 release 无 perf 符号)。
pub const E_PERF_UNAVAILABLE: &str = "E12004";

/// 兜底错误码（不属于任何分段）。
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

#[cfg(test)]
mod tests {
    use super::*;

    /// S5：外壳自有错误必须全在 `E12xxx` 段，且**互不相同**。
    ///
    /// 早先它们占着 `E11xxx`（`beam-config` 独占段），
    /// `E11003` 一个码两种含义（perf 不可用 vs 配置存储不可用），
    /// 前端按 `code` 查文案时无法定位。
    #[test]
    fn shell_codes_live_in_e12xxx_and_are_distinct() {
        let codes = [
            E_UNKNOWN_COMMAND,
            E_SHELL_CONFIG_INVALID,
            E_SHELL_IO,
            E_PERF_UNAVAILABLE,
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "外壳错误码必须互不相同");
        for code in codes {
            let num: u32 = code.trim_start_matches('E').parse().expect("错误码应为数字");
            assert!(
                (12000..13000).contains(&num),
                "{code} 不在外壳段 E12xxx 内"
            );
        }
    }

    /// `E11xxx` 归 `beam-config` 独占（§3.14），外壳不得占用。
    #[test]
    fn shell_does_not_use_the_config_segment() {
        for code in [
            E_UNKNOWN_COMMAND,
            E_SHELL_CONFIG_INVALID,
            E_SHELL_IO,
            E_PERF_UNAVAILABLE,
        ] {
            let num: u32 = code.trim_start_matches('E').parse().expect("错误码应为数字");
            assert!(
                !(11000..12000).contains(&num),
                "{code} 落在配置层 E11xxx 段里"
            );
        }
    }
}
