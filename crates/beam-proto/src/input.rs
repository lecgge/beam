//! 输入事件模型（§3.8：事件模型在协议层定义，主机侧负责注入）。
//!
//! §3.12 的 Android `core-input` 也把触控 / 手柄 / 遥控映射成本模块的 `InputEvent`，
//! 因此该模型必须与平台无关。
//!
//! **为什么全部用整数**：postcard 不支持浮点（见 §5 选型），
//! 因此坐标与摇杆量统一用定点数表示：
//! - 归一化坐标 `x` / `y`：`Q16.16`，屏幕宽/高各映射到 `0..=65535`；
//! - 滚轮 `dx` / `dy`：`Q16.16`；
//! - 手柄摇杆 `value`：`Q15`（`-32768..=32767`）。

use serde::{Deserialize, Serialize};

/// 鼠标按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    /// 左键。
    Left,
    /// 右键。
    Right,
    /// 中键。
    Middle,
    /// 侧键 1。
    X1,
    /// 侧键 2。
    X2,
}

/// 按键动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ButtonAction {
    /// 按下。
    Press,
    /// 抬起。
    Release,
}

/// 键盘动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyAction {
    /// 按下。
    Down,
    /// 抬起。
    Up,
}

/// 输入事件（§3.8 输入事件模型）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// 鼠标移动到归一化坐标（`Q16.16`）。
    MouseMove {
        /// 归一化横坐标。
        x: i32,
        /// 归一化纵坐标。
        y: i32,
    },
    /// 鼠标按键。
    MouseButton {
        /// 按键。
        button: MouseButton,
        /// 动作。
        action: ButtonAction,
    },
    /// 键盘按键（`code` 为 USB HID 扫描码）。
    Key {
        /// HID 扫描码。
        code: u16,
        /// 动作。
        action: KeyAction,
    },
    /// 滚轮（`Q16.16`）。
    Scroll {
        /// 水平位移。
        dx: i32,
        /// 垂直位移。
        dy: i32,
    },
    /// 手柄摇杆（`Q15`）。
    GamepadAxis {
        /// 手柄槽位。
        slot: u8,
        /// 轴编号。
        axis: u8,
        /// 轴值。
        value: i16,
    },
    /// 手柄按键。
    GamepadButton {
        /// 手柄槽位。
        slot: u8,
        /// 按键编号。
        button: u8,
        /// 动作。
        action: ButtonAction,
    },
}

impl InputEvent {
    /// 该事件是否属于手柄（用于按设备做权限控制，§3.8）。
    pub const fn is_gamepad(&self) -> bool {
        matches!(
            self,
            InputEvent::GamepadAxis { .. } | InputEvent::GamepadButton { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamepad_events_are_detected() {
        let axis = InputEvent::GamepadAxis {
            slot: 0,
            axis: 1,
            value: 1234,
        };
        assert!(axis.is_gamepad());
        let key = InputEvent::Key {
            code: 4,
            action: KeyAction::Down,
        };
        assert!(!key.is_gamepad());
    }
}
