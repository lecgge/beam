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
    /// `E10015`：解码纹理与交换链不匹配，画面搬不上去（**非文档码位**）。
    ///
    /// D3D11VA 视频解码的原生输出是 `DXGI_FORMAT_420Opaque`（4:2:0 双平面），
    /// 而普通桌面窗口的交换链是 `DXGI_FORMAT_B8G8R8A8_UNORM`。
    /// 两者格式不同 ⇒ 零拷贝不可行，必须先做格式转换。
    ///
    /// 早先这里**静默降级**成「清屏 + 照样走完整条呈现路径」：
    /// 不报错、不警告，用户看到空白窗口而上层以为一切正常。
    /// 现在如实报错，让上层能告诉用户「画面上不来」而不是假装在串流。
    pub const SOURCE_MISMATCH: u16 = 10015;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::DEVICE_LOST => "渲染设备丢失",
        code::SWAPCHAIN_FAILED => "交换链创建失败",
        code::FRAME_DROPPED => "画面帧已过期并被丢弃",
        code::BACKEND_DISABLED => "渲染后端未启用",
        code::SOURCE_MISMATCH => "解码画面与交换链格式不匹配,画面无法上屏",
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

    /// `E10015`：解码纹理与交换链不匹配，画面搬不上去。
    ///
    /// **这不是 §3.10 定义的码位**：它表示「当前构建缺少 4:2:0 → BGRA 的
    /// 转换路径」，而不是文档里列出的那几类渲染故障。
    #[error("E10015: 解码画面与交换链格式不匹配,画面无法上屏: {reason}")]
    SourceMismatch {
        /// 原因，仅用于日志。
        reason: String,
    },
}

impl RenderError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            RenderError::DeviceLost => code::DEVICE_LOST,
            RenderError::SwapChainFailed { .. } => code::SWAPCHAIN_FAILED,
            RenderError::FrameDropped => code::FRAME_DROPPED,
            RenderError::BackendDisabled => code::BACKEND_DISABLED,
            RenderError::SourceMismatch { .. } => code::SOURCE_MISMATCH,
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
        assert_eq!(
            RenderError::SourceMismatch { reason: "x".into() }.code(),
            10015
        );
        assert_eq!(message(10099), UNKNOWN_TEXT);
        assert!(is_render_code(10011));
    }
}
