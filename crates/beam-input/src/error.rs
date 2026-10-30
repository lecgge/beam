//! 输入层错误码与文案映射（§3.8 错误码段 `E8xxx`）。

/// 输入层错误码编号段 `E8xxx`（§3.8）。
pub mod code {
    /// `E8001`：ViGEmBus 未安装。
    pub const VIGEM_NOT_INSTALLED: u16 = 8001;
    /// `E8002`：权限拒绝。
    pub const PERMISSION_DENIED: u16 = 8002;
    /// `E8003`：输入后端未启用（§3.8 段内扩展）。
    pub const BACKEND_DISABLED: u16 = 8003;
    /// `E8004`：按键码无法映射为虚拟键（§3.8 段内扩展）。
    ///
    /// 协议侧 `InputEvent::Key.code` 是 USB HID 扫描码；注入前要翻译成 Win32
    /// 虚拟键码，翻译表里没有的键码就走这个错误（见 `sink.rs` 的 `hid_usage_to_vk`）。
    pub const UNSUPPORTED_KEY_CODE: u16 = 8004;
}

/// 由错误码反查人类可读文案（§3.11）。
pub fn message(code: u16) -> &'static str {
    match code {
        code::VIGEM_NOT_INSTALLED => "未检测到 ViGEmBus 驱动,手柄功能不可用",
        code::PERMISSION_DENIED => "没有注入输入的权限",
        code::BACKEND_DISABLED => "输入后端未启用",
        code::UNSUPPORTED_KEY_CODE => "按键码无法映射为虚拟键",
        _ => UNKNOWN_TEXT,
    }
}

/// 未知错误码的兜底文案。
pub const UNKNOWN_TEXT: &str = "未知输入错误";

/// 输入层错误（§3.8 错误码段 `E8xxx`）。
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// `E8001`：ViGEmBus 未安装。
    #[error("E8001: 未检测到 ViGEmBus 驱动")]
    ViGEmNotInstalled,

    /// `E8002`：权限拒绝。
    #[error("E8002: 没有注入输入的权限")]
    PermissionDenied,

    /// `E8003`：后端未启用。
    #[error("E8003: 输入后端未启用")]
    BackendDisabled,

    /// `E8004`：按键码无法映射为虚拟键。
    #[error("E8004: 按键码无法映射为虚拟键")]
    UnsupportedKeyCode,
}

impl InputError {
    /// 返回数字错误码。
    pub fn code(&self) -> u16 {
        match self {
            InputError::ViGEmNotInstalled => code::VIGEM_NOT_INSTALLED,
            InputError::PermissionDenied => code::PERMISSION_DENIED,
            InputError::BackendDisabled => code::BACKEND_DISABLED,
            InputError::UnsupportedKeyCode => code::UNSUPPORTED_KEY_CODE,
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
        matches!(self, InputError::ViGEmNotInstalled)
    }
}

/// 判断错误码是否属于输入层 `E8xxx` 段。
pub fn is_input_code(code: u16) -> bool {
    (8000..8100).contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_codes_are_stable() {
        assert_eq!(InputError::ViGEmNotInstalled.code(), 8001);
        assert_eq!(InputError::PermissionDenied.code(), 8002);
        assert_eq!(InputError::BackendDisabled.code(), 8003);
        assert_eq!(InputError::UnsupportedKeyCode.code(), 8004);
        assert_eq!(message(8099), UNKNOWN_TEXT);
        assert!(is_input_code(8001));
    }
}
