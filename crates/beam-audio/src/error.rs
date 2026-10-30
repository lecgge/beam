//! 音频层错误码与文案映射（§3.7 错误码段 `E7xxx`）。

/// 音频层错误码编号段 `E7xxx`（§3.7）。
pub mod code {
    /// `E7001`：音频设备打开失败。
    pub const DEVICE_FAILED: u16 = 7001;
    /// `E7002`：Opus 编解码失败。
    pub const CODEC_FAILED: u16 = 7002;
    /// `E7003`：音频输出失败。
    pub const OUTPUT_FAILED: u16 = 7003;
    /// `E7004`：音频后端未启用（§3.7 段内扩展）。
    pub const BACKEND_DISABLED: u16 = 7004;
    /// `E7005`：音频数据报非法（§3.7 段内扩展）。
    ///
    /// 音频数据报的线上格式问题：帧头解不开、类型不是音频、
    /// 分片参数不是「1 片 0 冗余」、或负载超过单数据报上限。
    pub const DATAGRAM_INVALID: u16 = 7005;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::DEVICE_FAILED => "音频设备打开失败",
        code::CODEC_FAILED => "音频编解码失败",
        code::OUTPUT_FAILED => "音频输出失败",
        code::BACKEND_DISABLED => "音频后端未启用",
        code::DATAGRAM_INVALID => "音频数据报非法",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知音频错误";

/// 音频层错误（§3.7 错误码段 `E7xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// `E7001`：设备打开失败。
    #[error("E7001: 音频设备打开失败: {reason}")]
    DeviceFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E7002`：编解码失败。
    #[error("E7002: Opus 编解码失败: {reason}")]
    CodecFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E7003`：输出失败。
    #[error("E7003: 音频输出失败: {reason}")]
    OutputFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E7004`：后端未启用。
    #[error("E7004: 音频后端未启用(缺少 opus feature)")]
    BackendDisabled,

    /// `E7005`：音频数据报非法。
    #[error("E7005: 音频数据报非法: {reason}")]
    DatagramInvalid {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl AudioError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            AudioError::DeviceFailed { .. } => code::DEVICE_FAILED,
            AudioError::CodecFailed { .. } => code::CODEC_FAILED,
            AudioError::OutputFailed { .. } => code::OUTPUT_FAILED,
            AudioError::BackendDisabled => code::BACKEND_DISABLED,
            AudioError::DatagramInvalid { .. } => code::DATAGRAM_INVALID,
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
            AudioError::DeviceFailed { .. } | AudioError::OutputFailed { .. }
        )
    }
}

/// 判断错误码是否属于音频层 `E7xxx` 段。
pub fn is_audio_code(code: u16) -> bool {
    (7000..7100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(AudioError::DeviceFailed { reason: "x".into() }.code(), 7001);
        assert_eq!(AudioError::CodecFailed { reason: "x".into() }.code(), 7002);
        assert_eq!(AudioError::OutputFailed { reason: "x".into() }.code(), 7003);
        assert_eq!(AudioError::BackendDisabled.code(), 7004);
        assert_eq!(
            AudioError::DatagramInvalid { reason: "x".into() }.code(),
            7005
        );
        assert_eq!(message(7099), UNKNOWN_TEXT);
        assert!(is_audio_code(7001));
    }
}
