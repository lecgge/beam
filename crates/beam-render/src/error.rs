//! 渲染层错误码与文案映射（§3.10 错误码段 `E10xxx`）。
//!
//! §3.10 把 decode / render 归在同一段 `E10xxx`，因此渲染层沿用 `100xx`，
//! 用 `1001x` 区间与解码层区分。

/// 渲染层错误码编号段 `E10xxx`（§3.10）。
pub mod code {
    /// `E10011`：渲染设备丢失。
    pub const DEVICE_LOST: u16 = 10011;
    /// `E10012`：交换链创建失败。
    pub const SWAPCHAIN_FAILED: u16 = 10012;
    /// `E10013`：帧被丢弃（超过上屏时限）。
    pub const FRAME_DROPPED: u16 = 10013;
    /// `E10014`：后端未启用（**非文档码位**，只表示后端没编译进来）。
    pub const BACKEND_DISABLED: u16 = 10014;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::DEVICE_LOST => "渲染设备丢失",
        code::SWAPCHAIN_FAILED => "交换链创建失败",
        code::FRAME_DROPPED => "画面帧已过期并被丢弃",
        code::BACKEND_DISABLED => "渲染后端未启用",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知渲染错误";

/// 渲染层错误（§3.10 错误码段 `E10xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// `E10011`：设备丢失。
    #[error("E10011: 渲染设备丢失")]
    DeviceLost,

    /// `E10012`：交换链创建失败。
    #[error("E10012: 交换链创建失败: {reason}")]
    SwapChainFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E10013`：帧被丢弃。
    #[error("E10013: 画面帧已过期并被丢弃")]
    FrameDropped,

    /// `E10014`：后端未启用。
    ///
    /// **这不是 §3.10 定义的码位**：它只表示当前构建没有编译进真实渲染后端
    /// （`d3d11` feature 未开启），与文档定义的 10011/10012/10013 无关。
    #[error("E10014: 渲染后端未启用(缺少 d3d11 feature)")]
    BackendDisabled,
}

impl RenderError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            RenderError::DeviceLost => code::DEVICE_LOST,
            RenderError::SwapChainFailed { .. } => code::SWAPCHAIN_FAILED,
            RenderError::FrameDropped => code::FRAME_DROPPED,
            RenderError::BackendDisabled => code::BACKEND_DISABLED,
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
            RenderError::DeviceLost
                | RenderError::FrameDropped
                | RenderError::SwapChainFailed { .. }
        )
    }
}

/// 判断错误码是否属于渲染层 `E10xxx` 段。
pub fn is_render_code(code: u16) -> bool {
    (10000..10100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(RenderError::DeviceLost.code(), 10011);
        assert_eq!(
            RenderError::SwapChainFailed { reason: "x".into() }.code(),
            10012
        );
        assert_eq!(RenderError::FrameDropped.code(), 10013);
        assert_eq!(RenderError::BackendDisabled.code(), 10014);
        assert_eq!(message(10099), UNKNOWN_TEXT);
        assert!(is_render_code(10011));
    }
}
