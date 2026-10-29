//! 发现层错误码与文案映射（§3.4 错误码段 `E4xxx`）。

/// 发现层错误码编号段 `E4xxx`（§3.4）。
pub mod code {
    /// `E4001`：mDNS 守护进程启动失败。
    pub const DAEMON_START_FAILED: u16 = 4001;
    /// `E4002`：服务注册失败。
    pub const REGISTER_FAILED: u16 = 4002;
    /// `E4003`：浏览启动失败。
    pub const BROWSE_FAILED: u16 = 4003;
    /// `E4004`：广播信息非法。
    pub const INVALID_INFO: u16 = 4004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::DAEMON_START_FAILED => "无法启动局域网发现服务",
        code::REGISTER_FAILED => "无法广播本机信息",
        code::BROWSE_FAILED => "无法浏览局域网主机",
        code::INVALID_INFO => "广播信息不合法",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知发现错误";

/// 发现层错误（§3.4 错误码段 `E4xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// `E4001`：mDNS 守护进程启动失败。
    #[error("E4001: 启动 mDNS 守护进程失败: {reason}")]
    DaemonStartFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E4002`：服务注册失败。
    #[error("E4002: 注册 mDNS 服务失败: {reason}")]
    RegisterFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E4003`：浏览启动失败。
    #[error("E4003: 浏览 mDNS 服务失败: {reason}")]
    BrowseFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E4004`：广播信息非法。
    #[error("E4004: 广播信息不合法: {reason}")]
    InvalidInfo {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl DiscoveryError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            DiscoveryError::DaemonStartFailed { .. } => code::DAEMON_START_FAILED,
            DiscoveryError::RegisterFailed { .. } => code::REGISTER_FAILED,
            DiscoveryError::BrowseFailed { .. } => code::BROWSE_FAILED,
            DiscoveryError::InvalidInfo { .. } => code::INVALID_INFO,
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
            DiscoveryError::DaemonStartFailed { .. }
                | DiscoveryError::RegisterFailed { .. }
                | DiscoveryError::BrowseFailed { .. }
        )
    }
}

/// 判断错误码是否属于发现层 `E4xxx` 段。
pub fn is_discovery_code(code: u16) -> bool {
    (4000..4100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            DiscoveryError::DaemonStartFailed { reason: "x".into() }.code(),
            4001
        );
        assert_eq!(
            DiscoveryError::RegisterFailed { reason: "x".into() }.code(),
            4002
        );
        assert_eq!(
            DiscoveryError::BrowseFailed { reason: "x".into() }.code(),
            4003
        );
        assert_eq!(message(4099), UNKNOWN_TEXT);
        assert!(is_discovery_code(4001));
    }
}
