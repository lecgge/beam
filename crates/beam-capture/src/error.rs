//! 抓屏层错误码与文案映射（§3.5 错误码段 `E5xxx`）。

/// 抓屏层错误码编号段 `E5xxx`（§3.5）。
pub mod code {
    /// `E5001`：无显示器 / 找不到目标显示器。
    pub const NO_DISPLAY: u16 = 5001;
    /// `E5002`：独占全屏无法抓取，应切到 DXGI 后端。
    pub const EXCLUSIVE_FULLSCREEN: u16 = 5002;
    /// `E5003`：设备丢失（D3D 设备被重置）。
    pub const DEVICE_LOST: u16 = 5003;
    /// `E5004`：抓屏后端未启用（feature 未开启，§3.5 段内扩展）。
    pub const BACKEND_DISABLED: u16 = 5004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::NO_DISPLAY => "没有找到可抓取的显示器",
        code::EXCLUSIVE_FULLSCREEN => "画面被独占全屏占用,已尝试切换抓屏方式",
        code::DEVICE_LOST => "图形设备丢失,需要重建抓屏管线",
        code::BACKEND_DISABLED => "抓屏后端未启用",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知抓屏错误";

/// 抓屏层错误（§3.5 错误码段 `E5xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum CapError {
    /// `E5001`：无显示器。
    #[error("E5001: 没有找到可抓取的显示器")]
    NoDisplay,

    /// `E5002`：独占全屏。
    #[error("E5002: 画面被独占全屏占用")]
    ExclusiveFullscreen,

    /// `E5003`：设备丢失。
    #[error("E5003: 图形设备丢失")]
    DeviceLost,

    /// `E5004`：后端未启用。
    #[error("E5004: 抓屏后端未启用(缺少 wgc feature)")]
    BackendDisabled,
}

impl CapError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            CapError::NoDisplay => code::NO_DISPLAY,
            CapError::ExclusiveFullscreen => code::EXCLUSIVE_FULLSCREEN,
            CapError::DeviceLost => code::DEVICE_LOST,
            CapError::BackendDisabled => code::BACKEND_DISABLED,
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
            CapError::NoDisplay | CapError::DeviceLost | CapError::ExclusiveFullscreen
        )
    }
}

/// 判断错误码是否属于抓屏层 `E5xxx` 段。
pub fn is_capture_code(code: u16) -> bool {
    (5000..5100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_codes_are_stable() {
        assert_eq!(CapError::NoDisplay.code(), 5001);
        assert_eq!(CapError::ExclusiveFullscreen.code(), 5002);
        assert_eq!(CapError::DeviceLost.code(), 5003);
        assert_eq!(CapError::BackendDisabled.code(), 5004);
        assert_eq!(message(5099), UNKNOWN_TEXT);
        assert!(is_capture_code(5001));
    }
}
