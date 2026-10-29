//! 抓屏层错误码与文案映射（§3.5 错误码段 `E5xxx`）。
//!
//! ## 码位口径
//!
//! §3.5 只定义了三个码位，真实抓屏路径的错误**必须**落到这三个码位上：
//!
//! | 码位 | 含义 | 触发场景 |
//! |---|---|---|
//! | `E5001` | 无显示器 | 目标显示器不存在 / 枚举不到显示器 / 抓屏目标创建失败 |
//! | `E5002` | 独占全屏无法抓取 | 画面被独占全屏应用占用，应切 DXGI 后端 |
//! | `E5003` | 设备丢失 | D3D 设备被重置 / 移除 / 桌面复制访问丢失 |
//!
//! `E5004` 是**本 crate 的段内扩展，不是文档定义的码位**：它只表示
//! 「抓屏后端未启用」（`wgc` feature 没开），文档 §3.5 没有定义这一情形。

/// 抓屏层错误码编号段 `E5xxx`（§3.5）。
pub mod code {
    /// `E5001`：无显示器 / 找不到目标显示器。
    pub const NO_DISPLAY: u16 = 5001;
    /// `E5002`：独占全屏无法抓取，应切到 DXGI 后端。
    pub const EXCLUSIVE_FULLSCREEN: u16 = 5002;
    /// `E5003`：设备丢失（D3D 设备被重置）。
    pub const DEVICE_LOST: u16 = 5003;
    /// `E5004`：抓屏后端未启用（feature 未开启）。
    ///
    /// **这不是 §3.5 定义的码位**，是本 crate 的段内扩展，
    /// 只用于「没有编译进真实抓屏后端」这一种情形。
    pub const BACKEND_DISABLED: u16 = 5004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::NO_DISPLAY => "没有找到可抓取的显示器",
        code::EXCLUSIVE_FULLSCREEN => "画面被独占全屏占用,需要改用 DXGI 抓屏",
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
    ///
    /// **这不是 §3.5 定义的码位**：它只表示当前构建没有编译进真实抓屏后端
    /// （`wgc` feature 未开启），与文档定义的 5001/5002/5003 无关。
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

    #[test]
    fn backend_disabled_is_flagged_as_non_doc_code() {
        // E5004 是段内扩展，不是文档定义的码位：文案与注释都必须说清楚。
        assert_eq!(CapError::BackendDisabled.text(), "抓屏后端未启用");
        assert!(CapError::BackendDisabled.text().contains("未启用"));
    }

    #[test]
    fn every_code_has_a_message() {
        for err in [
            CapError::NoDisplay,
            CapError::ExclusiveFullscreen,
            CapError::DeviceLost,
            CapError::BackendDisabled,
        ] {
            assert_ne!(err.text(), UNKNOWN_TEXT, "码位 {} 缺少文案", err.code());
            assert!(err.code_str().starts_with('E'));
        }
    }
}
