//! 网络层错误码与文案映射（§3.2 错误码段 `E2xxx`）。
//!
//! §6：各 crate 定义 thiserror 枚举并映射到 `E` 码；§3.11：UI 只根据 code 显示文案。

/// 网络层错误码编号段 `E2xxx`（§3.2）。
pub mod code {
    /// `E2001`：连接超时。
    pub const CONNECT_TIMEOUT: u16 = 2001;
    /// `E2002`：证书被拒绝（指纹不匹配）。
    pub const CERT_REJECTED: u16 = 2002;
    /// `E2003`：对端不支持 datagram。
    pub const DATAGRAM_UNSUPPORTED: u16 = 2003;
    /// `E2004`：帧过期丢弃。
    pub const FRAME_EXPIRED: u16 = 2004;
    /// `E2005`：分片重组失败（段内扩展：分片不完整且无法 FEC 恢复）。
    pub const REASSEMBLY_FAILED: u16 = 2005;
    /// `E2006`：FEC 不可恢复（段内扩展：冗余片不足以恢复）。
    pub const FEC_UNRECOVERABLE: u16 = 2006;
    /// `E2007`：缺少 TLS 身份材料（段内扩展：监听端未注入证书）。
    pub const MISSING_IDENTITY: u16 = 2007;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::CONNECT_TIMEOUT => "连接超时",
        code::CERT_REJECTED => "设备证书被拒绝",
        code::DATAGRAM_UNSUPPORTED => "对方不支持快速通道",
        code::FRAME_EXPIRED => "画面帧已过期并被丢弃",
        code::REASSEMBLY_FAILED => "画面分片重组失败",
        code::FEC_UNRECOVERABLE => "画面丢包过多,无法恢复",
        code::MISSING_IDENTITY => "缺少本机证书,无法开始监听",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知网络错误";

/// 网络层错误（§3.2 错误码段 `E2xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// `E2001`：连接超时。
    #[error("E2001: 连接超时({addr})")]
    ConnectTimeout {
        /// 目标地址。
        addr: String,
    },

    /// `E2002`：证书被拒绝。
    #[error("E2002: 证书被拒绝: {reason}")]
    CertRejected {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E2003`：对端不支持 datagram。
    #[error("E2003: 对端不支持 datagram")]
    DatagramUnsupported,

    /// `E2004`：帧过期丢弃。
    #[error("E2004: 帧 {frame_id} 过期并被丢弃")]
    FrameExpired {
        /// 过期帧号。
        frame_id: u32,
    },

    /// `E2005`：重组失败。
    #[error("E2005: 分片重组失败: {reason}")]
    ReassemblyFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E2006`：FEC 不可恢复。
    #[error("E2006: FEC 恢复失败: 缺失 {missing} 片,冗余 {parity} 片")]
    FecUnrecoverable {
        /// 缺失分片数。
        missing: usize,
        /// 可用冗余分片数。
        parity: usize,
    },

    /// `E2007`：缺少 TLS 身份材料。
    #[error("E2007: 缺少本机证书,无法开始监听")]
    MissingIdentity,

    /// `E2008`：底层 IO 错误（段内扩展）。
    #[error("E2008: 网络 IO 失败: {reason}")]
    Io {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl NetError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            NetError::ConnectTimeout { .. } => code::CONNECT_TIMEOUT,
            NetError::CertRejected { .. } => code::CERT_REJECTED,
            NetError::DatagramUnsupported => code::DATAGRAM_UNSUPPORTED,
            NetError::FrameExpired { .. } => code::FRAME_EXPIRED,
            NetError::ReassemblyFailed { .. } => code::REASSEMBLY_FAILED,
            NetError::FecUnrecoverable { .. } => code::FEC_UNRECOVERABLE,
            NetError::MissingIdentity => code::MISSING_IDENTITY,
            NetError::Io { .. } => 2008,
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
            NetError::ConnectTimeout { .. }
                | NetError::DatagramUnsupported
                | NetError::FrameExpired { .. }
                | NetError::ReassemblyFailed { .. }
                | NetError::FecUnrecoverable { .. }
                | NetError::Io { .. }
        )
    }
}

/// 判断错误码是否属于网络层 `E2xxx` 段。
pub fn is_net_code(code: u16) -> bool {
    (2000..2100).contains(&code)
}

impl From<beam_proto::ProtoError> for NetError {
    fn from(e: beam_proto::ProtoError) -> Self {
        NetError::ReassemblyFailed {
            reason: format!("协议层错误 {}: {e}", e.code_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_codes_are_stable() {
        assert_eq!(
            NetError::ConnectTimeout {
                addr: "1.2.3.4:1".into()
            }
            .code(),
            2001
        );
        assert_eq!(NetError::CertRejected { reason: "x".into() }.code(), 2002);
        assert_eq!(NetError::DatagramUnsupported.code(), 2003);
        assert_eq!(NetError::FrameExpired { frame_id: 1 }.code(), 2004);
    }

    #[test]
    fn unknown_code_has_fallback() {
        assert_eq!(message(2999), UNKNOWN_TEXT);
        assert!(is_net_code(2001));
        assert!(!is_net_code(3001));
    }
}
