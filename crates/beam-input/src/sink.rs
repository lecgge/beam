//! 输入注入契约与默认实现（§3.8）。
//!
//! - 主机侧：SendInput 注入键鼠、vigem-client 虚拟 Xbox 手柄、震动回传；
//! - 权限控制：按设备（[`crate::error::InputError::PermissionDenied`]）。
//! - 客户端侧采集不在本 crate（Windows 客户端由 beam-client 采集，Android 由 `core-input`）。
//!
//! ## 键码口径
//!
//! 协议侧 [`beam_proto::InputEvent::Key`] 的 `code` 是 **USB HID 扫描码**
//! （`beam-proto/src/input.rs`：「`code` 为 USB HID 扫描码」；Android 侧
//! `core-input/.../AndroidKeys.kt` 也写明「协议侧（`beam-proto::InputEvent::Key`）用的是
//! USB HID 扫描码（`u16`）…由主机端翻译成扫描码」）。
//!
//! Win32 注入要的是**虚拟键码**，所以本文件带一张 `HID_TO_VK` 翻译表
//! （HID Keyboard/Keypad page → `VIRTUAL_KEY`）。翻译表里没有的键码返回
//! [`crate::error::InputError::UnsupportedKeyCode`]（`E8004`），**不静默丢弃**。
//!
//! ## 构造与调用分离
//!
//! - [`build_inputs`]：纯构造 `INPUT` 序列，**不调用 `SendInput`**，可单测；
//! - [`send_inputs`]：真正调用 `SendInput`，**有真实副作用**（会真的按键 / 移动鼠标）。
//!
//! 单测只覆盖第一层，因此**不会**在测试机上真的注入按键。
//!
//! ## unsafe 策略（决策 D1）
//!
//! crate 根是 `#![deny(unsafe_code)]`，只在确实要碰 Win32 裸 API 的地方局部
//! `#[allow(unsafe_code)]`：
//!
//! - `send_inputs`：调用 `SendInput`（`windows` crate 暴露的是 `unsafe fn`）；
//! - 单测里的字段读取：`INPUT` 的 union 字段只能 unsafe 读。

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

/// 按设备的注入权限（§3.8「权限控制(按设备)」）。
///
/// **权限位从上层映射过来**，本 crate 不认识协议侧的对端权限、也不认识配置侧的
/// 输入设置（§3.8 的边界要求本层不依赖 `beam-config` / `beam-pair`）：
///
/// - 协议侧 `beam_pair::TrustedPeer.perms`（`PeerPerms{view,input,audio,gamepad}`）：
///   `input` 决定是否允许回传键鼠输入；`gamepad` 决定是否允许使用手柄；
/// - 配置侧 `beam_config::InputSettings{gamepad, allow_keyboard, allow_mouse}`：
///   `allow_keyboard` → `keyboard`，`allow_mouse` → `mouse`，`gamepad` → `gamepad`。
///
/// 映射由调用方（`beam-host`）完成，再通过 [`SendInputSink::set_permissions`] 交给注入器。
///
/// ## 默认值
///
/// [`Default`] 是**三类设备全部拒绝**（与旧版 `SendInputSink` 的 `allowed = false`
/// 行为一致）：新建注入器不会因为忘记配置权限而偷偷放行。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputPermissions {
    /// 允许注入键盘事件。
    pub keyboard: bool,
    /// 允许注入鼠标事件（含滚轮）。
    pub mouse: bool,
    /// 允许使用手柄。
    ///
    /// 手柄事件走 vigem 路径（[`GamepadHub`]），**不由 `SendInputSink` 注入**，
    /// 因此这个位在本注入器里只作记录，不据此拒绝事件。
    pub gamepad: bool,
}

impl InputPermissions {
    /// 三类设备全部允许。
    pub const fn all() -> Self {
        Self {
            keyboard: true,
            mouse: true,
            gamepad: true,
        }
    }

    /// 三类设备全部拒绝（等同于 [`Default`]）。
    pub const fn none() -> Self {
        Self {
            keyboard: false,
            mouse: false,
            gamepad: false,
        }
    }
}

/// 事件所属的设备类别（按设备做权限控制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceClass {
    /// 键盘。
    Keyboard,
    /// 鼠标（移动 / 按键 / 滚轮）。
    Mouse,
    /// 手柄（摇杆 / 按键）。
    Gamepad,
}

/// 事件所属的设备类别。
fn device_class(ev: &InputEvent) -> DeviceClass {
    match ev {
        InputEvent::Key { .. } => DeviceClass::Keyboard,
        InputEvent::MouseMove { .. }
        | InputEvent::MouseButton { .. }
        | InputEvent::Scroll { .. } => DeviceClass::Mouse,
        InputEvent::GamepadAxis { .. } | InputEvent::GamepadButton { .. } => DeviceClass::Gamepad,
    }
}

/// SendInput 键鼠注入器。
#[derive(Debug, Default)]
pub struct SendInputSink {
    /// 按设备的注入权限。
    perms: InputPermissions,
}

impl SendInputSink {
    /// 新建注入器（默认三类设备都不允许注入）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置按设备的注入权限（§3.8）。
    pub fn set_permissions(&mut self, perms: InputPermissions) {
        self.perms = perms;
    }

    /// 读取当前按设备的注入权限。
    pub fn permissions(&self) -> InputPermissions {
        self.perms
    }

    /// 设置注入权限（**兼容旧接口**：等价于三类设备同时开关）。
    ///
    /// `true` → [`InputPermissions::all`]，`false` → [`InputPermissions::none`]。
    /// 需要按设备区分时用 [`SendInputSink::set_permissions`]。
    pub fn set_allowed(&mut self, allowed: bool) {
        self.perms = if allowed {
            InputPermissions::all()
        } else {
            InputPermissions::none()
        };
    }

    /// 该事件当前是否被允许注入（按设备）。
    ///
    /// 手柄恒为「允许」：它不由本注入器注入（走 vigem 路径）。
    fn permits(&self, ev: &InputEvent) -> bool {
        match device_class(ev) {
            DeviceClass::Keyboard => self.perms.keyboard,
            DeviceClass::Mouse => self.perms.mouse,
            DeviceClass::Gamepad => true,
        }
    }
}

impl InputSink for SendInputSink {
    fn apply(&mut self, ev: InputEvent) -> Result<(), InputError> {
        // 手柄事件**不走 SendInput**（vigem 路径，见 `GamepadHub`）：
        // 保持现状语义 —— 不注入、也不报错。
        if ev.is_gamepad() {
            return Ok(());
        }
        // 按设备做权限控制（§3.8）。
        if !self.permits(&ev) {
            return Err(InputError::PermissionDenied);
        }
        #[cfg(windows)]
        {
            let inputs = build_inputs(&ev)?;
            if inputs.is_empty() {
                return Ok(());
            }
            send_inputs(&inputs)
        }
        #[cfg(not(windows))]
        {
            // 本 crate 是 Windows only 平台 crate：非 Windows 上没有 SendInput 后端。
            let _ = ev;
            Ok(())
        }
    }
}

#[cfg(windows)]
mod windows_inject {
    use super::*;
    use beam_proto::{ButtonAction, KeyAction, MouseButton};
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    /// 侧键 1（`XBUTTON1`）。
    const XBUTTON1: u32 = 0x0001;
    /// 侧键 2（`XBUTTON2`）。
    const XBUTTON2: u32 = 0x0002;

    /// 把输入事件翻译成 Win32 `INPUT` 序列（**只构造，不注入**）。
    ///
    /// 单测只覆盖这一层：它不调用 `SendInput`，因此不会真的按下按键。
    ///
    /// 返回空 `Vec` 表示「这个事件不需要（也不能）用 `SendInput` 表达」——
    /// 目前只有手柄事件会走到这里（手柄走 vigem 路径）。
    pub(super) fn build_inputs(ev: &InputEvent) -> Result<Vec<INPUT>, InputError> {
        match ev {
            InputEvent::MouseMove { x, y } => {
                // **绝对**移动：协议层 `InputEvent::MouseMove` 的字段文档是
                // 「鼠标移动到归一化坐标（`Q16.16`）」（`beam-proto/src/input.rs:50-55`），
                // 模块头进一步写明「屏幕宽/高各映射到 `0..=65535`」
                // （`beam-proto/src/input.rs:8`）—— 这正是 Win32
                // `MOUSEEVENTF_ABSOLUTE` 的坐标口径（0..=65535 映射到屏幕）。
                //
                // 因此这里用绝对坐标，**不是**相对移动（`MOUSEEVENTF_MOVE`）。
                // 相对移动在协议层没有表达力：客户端只知道「移动到哪」，
                // 不知道「从哪移到哪」—— 增量需要客户端知道本机当前指针位置，
                // 而那是**客户端**的状态，不该由主机猜。
                //
                // 不加 `MOUSEEVENTF_VIRTUALDESK`：协议说的是「屏幕」，
                // 不是虚拟桌面（多显示器下 MOUSEEVENTF_ABSOLUTE 默认映射到主显示器）。
                Ok(vec![mouse_input(*x, *y, MOUSEEVENTF_ABSOLUTE, 0)])
            }
            InputEvent::MouseButton { button, action } => {
                let (flags, data) = mouse_button_flags(*button, *action);
                Ok(vec![mouse_input(0, 0, flags, data)])
            }
            InputEvent::Key { code, action } => {
                // 协议给的是 USB HID 扫描码，Win32 要的是虚拟键码。
                let vk = hid_usage_to_vk(*code).ok_or(InputError::UnsupportedKeyCode)?;
                let flags = match action {
                    KeyAction::Down => KEYBD_EVENT_FLAGS(0),
                    KeyAction::Up => KEYEVENTF_KEYUP,
                };
                Ok(vec![INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: vk,
                            wScan: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                }])
            }
            InputEvent::Scroll { dx, dy } => {
                // `MOUSEEVENTF_WHEEL`（纵向）/ `MOUSEEVENTF_HWHEEL`（横向）。
                //
                // 协议给的是 `Q16.16` 位移（`beam-proto/src/input.rs:71-77`），
                // Win32 的 `dwData` 要的是**滚轮刻度**（`WHEEL_DELTA` = 120 一格）。
                //
                // 换算口径：**Q16.16 的整数部分 = 刻度数**，小数部分是高精度滚轮
                // 的细分，Win32 的刻度粒度就是 120，所以小数部分不参与换算。
                // （协议与 §3.8 都没有规定滚轮的物理单位，这是**实现口径**，
                // 已在差距清单里登记待确认。）
                const WHEEL_DELTA: i32 = 120;
                let mut out = Vec::new();
                if *dy != 0 {
                    let notches = dy >> 16;
                    out.push(mouse_input(
                        0,
                        0,
                        MOUSEEVENTF_WHEEL,
                        (notches * WHEEL_DELTA) as u32,
                    ));
                }
                if *dx != 0 {
                    let notches = dx >> 16;
                    out.push(mouse_input(
                        0,
                        0,
                        MOUSEEVENTF_HWHEEL,
                        (notches * WHEEL_DELTA) as u32,
                    ));
                }
                Ok(out)
            }
            // 手柄不走 SendInput。
            InputEvent::GamepadAxis { .. } | InputEvent::GamepadButton { .. } => Ok(Vec::new()),
        }
    }

    /// 构造一个鼠标 `INPUT`。
    fn mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS, data: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    /// 鼠标按键 → (`dwFlags`, `dwData`)。
    fn mouse_button_flags(button: MouseButton, action: ButtonAction) -> (MOUSE_EVENT_FLAGS, u32) {
        match (button, action) {
            (MouseButton::Left, ButtonAction::Press) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, ButtonAction::Release) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, ButtonAction::Press) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, ButtonAction::Release) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, ButtonAction::Press) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, ButtonAction::Release) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::X1, ButtonAction::Press) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::X1, ButtonAction::Release) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::X2, ButtonAction::Press) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::X2, ButtonAction::Release) => (MOUSEEVENTF_XUP, XBUTTON2),
        }
    }

    /// 真正调用 Win32 `SendInput`（**有真实副作用**：会真的按键 / 移动鼠标）。
    ///
    /// 返回 0 表示注入失败，最常见的原因是 UIPI（目标窗口权限更高），
    /// 映射成 [`InputError::PermissionDenied`]（`E8002`）。
    #[allow(unsafe_code)]
    pub(super) fn send_inputs(inputs: &[INPUT]) -> Result<(), InputError> {
        // SAFETY: `inputs` 是合法的 `INPUT` 切片，`cbSize` 传 `size_of::<INPUT>()`；
        // `SendInput` 只在该调用期间读取这块内存，不保留指针。
        let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        if sent != inputs.len() as u32 {
            tracing::warn!(
                sent,
                expected = inputs.len(),
                "SendInput 注入失败（返回 0 通常是 UIPI：目标窗口权限更高）"
            );
            return Err(InputError::PermissionDenied);
        }
        Ok(())
    }

    /// USB HID 用法 ID → Win32 虚拟键码。
    ///
    /// 表覆盖 HID Keyboard/Keypad page 的标准键盘部分（字母 / 数字 / 符号 /
    /// 功能键 / 导航键 / 小键盘 / 修饰键）。表里没有的用法返回 `None`。
    pub(super) fn hid_usage_to_vk(code: u16) -> Option<VIRTUAL_KEY> {
        HID_TO_VK
            .binary_search_by_key(&code, |(hid, _)| *hid)
            .ok()
            .map(|idx| HID_TO_VK[idx].1)
    }

    /// USB HID 用法 ID（Keyboard/Keypad page）→ Win32 `VIRTUAL_KEY`。
    ///
    /// **必须按 HID 码严格升序**（`binary_search_by_key` 的前提），且不重复。
    pub(super) const HID_TO_VK: &[(u16, VIRTUAL_KEY)] = &[
        (0x04, VK_A),
        (0x05, VK_B),
        (0x06, VK_C),
        (0x07, VK_D),
        (0x08, VK_E),
        (0x09, VK_F),
        (0x0A, VK_G),
        (0x0B, VK_H),
        (0x0C, VK_I),
        (0x0D, VK_J),
        (0x0E, VK_K),
        (0x0F, VK_L),
        (0x10, VK_M),
        (0x11, VK_N),
        (0x12, VK_O),
        (0x13, VK_P),
        (0x14, VK_Q),
        (0x15, VK_R),
        (0x16, VK_S),
        (0x17, VK_T),
        (0x18, VK_U),
        (0x19, VK_V),
        (0x1A, VK_W),
        (0x1B, VK_X),
        (0x1C, VK_Y),
        (0x1D, VK_Z),
        (0x1E, VK_1),
        (0x1F, VK_2),
        (0x20, VK_3),
        (0x21, VK_4),
        (0x22, VK_5),
        (0x23, VK_6),
        (0x24, VK_7),
        (0x25, VK_8),
        (0x26, VK_9),
        (0x27, VK_0),
        (0x28, VK_RETURN),
        (0x29, VK_ESCAPE),
        (0x2A, VK_BACK),
        (0x2B, VK_TAB),
        (0x2C, VK_SPACE),
        (0x2D, VK_OEM_MINUS),
        (0x2E, VK_OEM_PLUS),
        (0x2F, VK_OEM_4),
        (0x30, VK_OEM_6),
        (0x31, VK_OEM_5),
        (0x32, VK_OEM_1),
        (0x33, VK_OEM_7),
        (0x34, VK_OEM_3),
        (0x35, VK_OEM_COMMA),
        (0x36, VK_OEM_PERIOD),
        (0x37, VK_OEM_2),
        (0x38, VK_CAPITAL),
        (0x39, VK_F1),
        (0x3A, VK_F2),
        (0x3B, VK_F3),
        (0x3C, VK_F4),
        (0x3D, VK_F5),
        (0x3E, VK_F6),
        (0x3F, VK_F7),
        (0x40, VK_F8),
        (0x41, VK_F9),
        (0x42, VK_F10),
        (0x43, VK_F11),
        (0x44, VK_F12),
        (0x45, VK_SNAPSHOT),
        (0x46, VK_SCROLL),
        (0x47, VK_PAUSE),
        (0x48, VK_INSERT),
        (0x49, VK_HOME),
        (0x4A, VK_PRIOR),
        (0x4B, VK_DELETE),
        (0x4C, VK_END),
        (0x4D, VK_NEXT),
        (0x4E, VK_RIGHT),
        (0x4F, VK_LEFT),
        (0x50, VK_DOWN),
        (0x51, VK_UP),
        (0x52, VK_NUMLOCK),
        (0x53, VK_DIVIDE),
        (0x54, VK_MULTIPLY),
        (0x55, VK_SUBTRACT),
        (0x56, VK_ADD),
        (0x57, VK_RETURN),
        (0x58, VK_NUMPAD1),
        (0x59, VK_NUMPAD2),
        (0x5A, VK_NUMPAD3),
        (0x5B, VK_NUMPAD4),
        (0x5C, VK_NUMPAD5),
        (0x5D, VK_NUMPAD6),
        (0x5E, VK_NUMPAD7),
        (0x5F, VK_NUMPAD8),
        (0x60, VK_NUMPAD9),
        (0x61, VK_NUMPAD0),
        (0x62, VK_DECIMAL),
        (0x63, VK_OEM_102),
        (0x64, VK_APPS),
        (0x66, VK_OEM_PLUS),
        (0xE0, VK_CONTROL),
        (0xE1, VK_SHIFT),
        (0xE2, VK_MENU),
        (0xE3, VK_LWIN),
        (0xE4, VK_CONTROL),
        (0xE5, VK_SHIFT),
        (0xE6, VK_MENU),
        (0xE7, VK_RWIN),
    ];
}

#[cfg(windows)]
use windows_inject::{build_inputs, send_inputs};

#[cfg(test)]
mod tests {
    use super::*;
    use beam_proto::{ButtonAction, KeyAction, MouseButton};

    // ---------------------------------------------------------------------
    // 权限（跨平台）。这些用例**不会**触发真实注入：
    // 只有「被拒绝」的事件才会走到 `apply`，它们在构造之前就返回了。
    // ---------------------------------------------------------------------

    #[test]
    fn new_sink_denies_everything() {
        let sink = SendInputSink::new();
        assert!(!sink.permits(&InputEvent::Key {
            code: 4,
            action: KeyAction::Down
        }));
        assert!(!sink.permits(&InputEvent::MouseMove { x: 1, y: 2 }));
        assert!(!sink.permits(&InputEvent::Scroll { dx: 0, dy: 0 }));
    }

    #[test]
    fn permission_is_checked_per_device() {
        let mut sink = SendInputSink::new();
        sink.set_permissions(InputPermissions {
            keyboard: true,
            mouse: false,
            gamepad: false,
        });
        assert!(sink.permits(&InputEvent::Key {
            code: 4,
            action: KeyAction::Down
        }));
        assert!(!sink.permits(&InputEvent::MouseMove { x: 0, y: 0 }));
        assert!(!sink.permits(&InputEvent::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Press
        }));
        assert!(!sink.permits(&InputEvent::Scroll { dx: 0, dy: 0 }));
    }

    #[test]
    fn set_allowed_toggles_all_devices() {
        let mut sink = SendInputSink::new();
        sink.set_allowed(true);
        assert_eq!(sink.permissions(), InputPermissions::all());
        sink.set_allowed(false);
        assert_eq!(sink.permissions(), InputPermissions::none());
    }

    #[test]
    fn denied_key_event_is_rejected_before_injection() {
        let mut sink = SendInputSink::new();
        let err = sink
            .apply(InputEvent::Key {
                code: 4,
                action: KeyAction::Down,
            })
            .expect_err("无权限必须失败");
        assert_eq!(err.code(), 8002);
    }

    #[test]
    fn denied_mouse_event_is_rejected_before_injection() {
        let mut sink = SendInputSink::new();
        let err = sink
            .apply(InputEvent::MouseButton {
                button: MouseButton::Left,
                action: ButtonAction::Press,
            })
            .expect_err("无权限必须失败");
        assert_eq!(err.code(), 8002);
    }

    #[test]
    fn denied_scroll_event_is_rejected_before_injection() {
        let mut sink = SendInputSink::new();
        let err = sink
            .apply(InputEvent::Scroll { dx: 0, dy: 120 })
            .expect_err("无权限必须失败");
        assert_eq!(err.code(), 8002);
    }

    #[test]
    fn gamepad_event_is_never_injected_and_never_fails() {
        // 手柄走 vigem 路径：不注入、也不报错（即使权限位是关的）。
        let mut sink = SendInputSink::new();
        assert!(sink
            .apply(InputEvent::GamepadAxis {
                slot: 0,
                axis: 0,
                value: 0,
            })
            .is_ok());
        assert!(sink
            .apply(InputEvent::GamepadButton {
                slot: 0,
                button: 0,
                action: ButtonAction::Press,
            })
            .is_ok());
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

    // ---------------------------------------------------------------------
    // 构造层（Windows only）。
    //
    // 这些用例只调用 `build_inputs`（纯构造），**绝不**调用 `send_inputs`，
    // 因此不会在测试机上真的注入按键 / 移动鼠标。
    // ---------------------------------------------------------------------
    #[cfg(windows)]
    mod build_inputs_tests {
        use super::super::windows_inject::{build_inputs, hid_usage_to_vk, HID_TO_VK};
        use beam_proto::{ButtonAction, InputEvent, KeyAction, MouseButton};
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE,
            MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
            MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, VK_A, VK_F1, VK_LEFT,
        };

        /// 读取键盘 `INPUT` 的 (`wVk`, `wScan`, `dwFlags`)。
        #[allow(unsafe_code)]
        fn key_fields(input: &INPUT) -> Option<(u16, u16, u32)> {
            if input.r#type != INPUT_KEYBOARD {
                return None;
            }
            // SAFETY: `r#type` 已确认是 `INPUT_KEYBOARD`，union 的 `ki` 字段有效。
            let ki = unsafe { input.Anonymous.ki };
            Some((ki.wVk.0, ki.wScan, ki.dwFlags.0))
        }

        /// 读取鼠标 `INPUT` 的 (`dx`, `dy`, `mouseData`, `dwFlags`)。
        #[allow(unsafe_code)]
        fn mouse_fields(input: &INPUT) -> Option<(i32, i32, u32, u32)> {
            if input.r#type != INPUT_MOUSE {
                return None;
            }
            // SAFETY: `r#type` 已确认是 `INPUT_MOUSE`，union 的 `mi` 字段有效。
            let mi = unsafe { input.Anonymous.mi };
            Some((mi.dx, mi.dy, mi.mouseData, mi.dwFlags.0))
        }

        #[test]
        fn key_down_maps_hid_usage_to_vk() {
            // HID 0x04 = 'a'。
            let inputs = build_inputs(&InputEvent::Key {
                code: 0x04,
                action: KeyAction::Down,
            })
            .expect("HID 0x04 应当能映射");
            assert_eq!(inputs.len(), 1);
            let (vk, scan, flags) = key_fields(&inputs[0]).expect("应当是键盘事件");
            assert_eq!(vk, VK_A.0);
            assert_eq!(scan, 0, "走 wVk 就不带 wScan");
            assert_eq!(flags & KEYEVENTF_KEYUP.0, 0, "按下不能带 KEYUP");
        }

        #[test]
        fn key_up_sets_keyup_flag() {
            // HID 0x4F = 左方向键。
            let inputs = build_inputs(&InputEvent::Key {
                code: 0x4F,
                action: KeyAction::Up,
            })
            .expect("HID 0x4F 应当能映射");
            let (vk, _scan, flags) = key_fields(&inputs[0]).expect("应当是键盘事件");
            assert_eq!(vk, VK_LEFT.0);
            assert_ne!(flags & KEYEVENTF_KEYUP.0, 0, "抬起必须带 KEYUP");
        }

        #[test]
        fn unmapped_hid_code_is_reported_not_dropped() {
            // `INPUT` 没有 `Debug`，所以不能用 `expect_err`。
            let err = match build_inputs(&InputEvent::Key {
                code: 0xFFFF,
                action: KeyAction::Down,
            }) {
                Ok(_) => panic!("未知键码必须报错"),
                Err(e) => e,
            };
            assert_eq!(err.code(), 8004);
        }

        #[test]
        fn mouse_move_is_an_absolute_move() {
            // 协议给的是归一化绝对坐标（0..=65535 映射到屏幕），
            // 对应 Win32 的 MOUSEEVENTF_ABSOLUTE，不是相对移动。
            let inputs = build_inputs(&InputEvent::MouseMove { x: 7, y: 9 }).expect("应当能构造");
            assert_eq!(inputs.len(), 1);
            let (dx, dy, _data, flags) = mouse_fields(&inputs[0]).expect("应当是鼠标事件");
            assert_eq!((dx, dy), (7, 9));
            assert_ne!(flags & MOUSEEVENTF_ABSOLUTE.0, 0);
            assert_eq!(flags & MOUSEEVENTF_MOVE.0, 0, "不能同时是相对移动");
        }

        #[test]
        fn mouse_button_press_and_release_use_opposite_flags() {
            let down = build_inputs(&InputEvent::MouseButton {
                button: MouseButton::Left,
                action: ButtonAction::Press,
            })
            .expect("应当能构造");
            let up = build_inputs(&InputEvent::MouseButton {
                button: MouseButton::Left,
                action: ButtonAction::Release,
            })
            .expect("应当能构造");
            let (_, _, _, down_flags) = mouse_fields(&down[0]).expect("应当是鼠标事件");
            let (_, _, _, up_flags) = mouse_fields(&up[0]).expect("应当是鼠标事件");
            assert_ne!(down_flags & MOUSEEVENTF_LEFTDOWN.0, 0);
            assert_ne!(up_flags & MOUSEEVENTF_LEFTUP.0, 0);

            let right = build_inputs(&InputEvent::MouseButton {
                button: MouseButton::Right,
                action: ButtonAction::Press,
            })
            .expect("应当能构造");
            let (_, _, _, right_flags) = mouse_fields(&right[0]).expect("应当是鼠标事件");
            assert_ne!(right_flags & MOUSEEVENTF_RIGHTDOWN.0, 0);
        }

        #[test]
        fn side_button_carries_xbutton_data() {
            let inputs = build_inputs(&InputEvent::MouseButton {
                button: MouseButton::X1,
                action: ButtonAction::Press,
            })
            .expect("应当能构造");
            let (_, _, data, flags) = mouse_fields(&inputs[0]).expect("应当是鼠标事件");
            assert_ne!(flags & MOUSEEVENTF_XDOWN.0, 0);
            assert_eq!(data, 1, "X1 = XBUTTON1");
        }

        #[test]
        fn scroll_converts_q16_to_wheel_notches() {
            // 协议给的是 Q16.16 位移；整数部分 = 刻度数，Win32 一格 = 120。
            let inputs =
                build_inputs(&InputEvent::Scroll { dx: 0, dy: -65536 }).expect("应当能构造");
            assert_eq!(inputs.len(), 1);
            let (_, _, data, flags) = mouse_fields(&inputs[0]).expect("应当是鼠标事件");
            assert_ne!(flags & MOUSEEVENTF_WHEEL.0, 0);
            assert_eq!(data as i32, -120, "一格滚轮 = -120（向上滚是负）");

            let horizontal =
                build_inputs(&InputEvent::Scroll { dx: 65536, dy: 0 }).expect("应当能构造");
            let (_, _, h_data, h_flags) = mouse_fields(&horizontal[0]).expect("应当是鼠标事件");
            assert_ne!(h_flags & MOUSEEVENTF_HWHEEL.0, 0);
            assert_eq!(h_data as i32, 120);

            // 小数部分（亚刻度）不参与换算：Win32 的刻度粒度就是 120。
            let fractional = build_inputs(&InputEvent::Scroll {
                dx: 0,
                dy: 65536 + 32768,
            })
            .expect("应当能构造");
            let (_, _, f_data, _) = mouse_fields(&fractional[0]).expect("应当是鼠标事件");
            assert_eq!(f_data as i32, 120);
        }

        #[test]
        fn gamepad_event_builds_nothing() {
            let inputs = build_inputs(&InputEvent::GamepadAxis {
                slot: 0,
                axis: 0,
                value: 0,
            })
            .expect("手柄事件不报错");
            assert!(inputs.is_empty(), "手柄不走 SendInput");
        }

        #[test]
        fn hid_table_is_sorted_and_unique() {
            // 二分查找的前提：按 HID 码严格升序且不重复。
            for pair in HID_TO_VK.windows(2) {
                assert!(
                    pair[0].0 < pair[1].0,
                    "HID 表必须严格升序: {:04X} 之后是 {:04X}",
                    pair[0].0,
                    pair[1].0
                );
            }
        }

        #[test]
        fn hid_usage_maps_to_expected_vk() {
            assert_eq!(hid_usage_to_vk(0x04).map(|vk| vk.0), Some(VK_A.0));
            assert_eq!(hid_usage_to_vk(0x39).map(|vk| vk.0), Some(VK_F1.0));
            assert_eq!(hid_usage_to_vk(0xFFFF), None);
        }
    }
}
