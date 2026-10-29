//! 输入注入契约与默认实现（§3.8）。
//!
//! - 主机侧：SendInput 注入键鼠、vigem-client 虚拟 Xbox 手柄、震动回传；
//! - 权限控制：按设备（[`crate::error::InputError::PermissionDenied`]）。
//! - 客户端侧采集不在本 crate（Windows 客户端由 beam-client 采集，Android 由 `core-input`）。

use beam_proto::InputEvent;

use crate::error::InputError;

/// 输入注入器（§3.8 `InputSink`）。
pub trait InputSink: Send {
    /// 注入一个输入事件。
    fn apply(&mut self, ev: InputEvent) -> Result<(), InputError>;
}

/// 手柄槽位数量上限（Xbox 手柄最多 4 个）。
pub const MAX_GAMEPAD_SLOTS: u8 = 4;

/// 手柄句柄（§3.8 `GamepadHub::attach(slot) -> GamepadHandle`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GamepadHandle {
    /// 槽位号。
    pub slot: u8,
}

impl GamepadHandle {
    /// 构造句柄。
    pub const fn new(slot: u8) -> Self {
        Self { slot }
    }
}

/// 震动回传（§3.8）。
pub trait RumbleSink: Send {
    /// 让指定手柄震动。
    fn rumble(&mut self, handle: GamepadHandle, strong: u16, weak: u16) -> Result<(), InputError>;
}

/// 手柄中心（§3.8 `GamepadHub`）。
#[derive(Debug, Default)]
pub struct GamepadHub {
    attached: Vec<GamepadHandle>,
    /// 与 ViGEmBus 的连接（`vigem` feature 开启时有效）。
    #[cfg(feature = "vigem")]
    client: Option<vigem_client::Client>,
}

impl GamepadHub {
    /// 新建手柄中心。
    pub fn new() -> Self {
        Self::default()
    }

    /// 挂载一个虚拟手柄（§3.8 `GamepadHub::attach(slot)`）。
    ///
    /// ViGEmBus 未安装时返回 `E8001`；槽位越界返回 `E8002`。
    pub fn attach(&mut self, slot: u8) -> Result<GamepadHandle, InputError> {
        if slot >= MAX_GAMEPAD_SLOTS {
            return Err(InputError::PermissionDenied);
        }
        if let Some(existing) = self.attached.iter().find(|h| h.slot == slot) {
            return Ok(*existing);
        }
        #[cfg(feature = "vigem")]
        {
            // 真实实现的第一步：连接 ViGEmBus。驱动未安装 → E8001。
            // X360 目标的创建与震动回传在 M3（v1）落地。
            if self.client.is_none() {
                let client =
                    vigem_client::Client::connect().map_err(|_| InputError::ViGEmNotInstalled)?;
                self.client = Some(client);
            }
            let handle = GamepadHandle::new(slot);
            self.attached.push(handle);
            Ok(handle)
        }
        #[cfg(not(feature = "vigem"))]
        {
            Err(InputError::ViGEmNotInstalled)
        }
    }

    /// 卸载手柄。
    pub fn detach(&mut self, slot: u8) -> bool {
        let before = self.attached.len();
        self.attached.retain(|h| h.slot != slot);
        self.attached.len() != before
    }

    /// 已挂载的手柄。
    pub fn attached(&self) -> &[GamepadHandle] {
        &self.attached
    }
}

/// SendInput 键鼠注入器。
#[derive(Debug, Default)]
pub struct SendInputSink {
    /// 是否已获得注入权限。
    allowed: bool,
}

impl SendInputSink {
    /// 新建注入器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置注入权限（§3.8 按设备的权限控制）。
    pub fn set_allowed(&mut self, allowed: bool) {
        self.allowed = allowed;
    }
}

impl InputSink for SendInputSink {
    fn apply(&mut self, ev: InputEvent) -> Result<(), InputError> {
        if !self.allowed {
            return Err(InputError::PermissionDenied);
        }
        #[cfg(windows)]
        {
            let _ = ev;
            // 真实实现：构造 INPUT 结构并调用 SendInput。
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = ev;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_proto::{ButtonAction, KeyAction, MouseButton};

    #[test]
    fn sink_requires_permission() {
        let mut sink = SendInputSink::new();
        let ev = InputEvent::Key {
            code: 4,
            action: KeyAction::Down,
        };
        let err = sink.apply(ev.clone()).expect_err("无权限必须失败");
        assert_eq!(err.code(), 8002);

        sink.set_allowed(true);
        assert!(sink.apply(ev).is_ok());
    }

    #[test]
    fn gamepad_attach_respects_slot_limit() {
        let mut hub = GamepadHub::new();
        // 槽位越界始终是 E8002。
        assert_eq!(hub.attach(9).unwrap_err().code(), 8002);
        // 是否能挂载取决于本机是否装了 ViGEmBus。
        match hub.attach(0) {
            Ok(h) => assert_eq!(h.slot, 0),
            Err(e) => assert_eq!(e.code(), 8001),
        }
    }

    #[test]
    fn gamepad_attach_and_detach() {
        let mut hub = GamepadHub::new();
        let Ok(handle) = hub.attach(0) else {
            // 本机没有 ViGEmBus，跳过挂载 / 卸载断言。
            return;
        };
        assert_eq!(handle.slot, 0);
        assert_eq!(hub.attached().len(), 1);
        assert!(hub.detach(0));
        assert!(hub.attached().is_empty());
    }

    #[test]
    fn mouse_button_event_is_accepted() {
        let mut sink = SendInputSink::new();
        sink.set_allowed(true);
        let ev = InputEvent::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Press,
        };
        assert!(sink.apply(ev).is_ok());
    }
}
