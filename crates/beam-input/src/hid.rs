//! USB HID 用法 ID ↔ Win32 虚拟键码（§3.8 的键码口径）。
//!
//! ## 为什么两边要共用**同一张表**
//!
//! 协议侧 [`beam_proto::InputEvent::Key`] 的 `code` 是 **USB HID 用法 ID**
//! （`beam-proto/src/input.rs`：「`code` 为 USB HID 扫描码」；Android 侧
//! `core-input/.../AndroidKeys.kt` 也写明「协议侧用的是 USB HID 扫描码…由主机端翻译成扫描码」）。
//!
//! - **主机侧注入**需要 HID → 虚拟键码；
//! - **客户端侧采集**需要 虚拟键码 → HID（低级钩子给的是 `vkCode`）。
//!
//! 两边各写一张表，迟早会漂移（一边加了键、另一边没有 ⇒ 按这个键没反应）。
//! 所以这张表只在这里定义一次，两边都从这里取。
//!
//! ## 表的内容
//!
//! 覆盖 HID Keyboard/Keypad page 的标准键盘部分（字母 / 数字 / 符号 /
//! 功能键 / 导航键 / 小键盘 / 修饰键）。表里没有的用法返回 `None` ——
//! **不静默丢弃**，由调用方决定怎么办（注入侧报 `E8004`；采集侧跳过该键）。

use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// HID 用法 ID → Win32 虚拟键码。
///
/// 表里没有的用法返回 `None`。
#[cfg(windows)]
pub fn hid_to_vk(code: u16) -> Option<u32> {
    HID_TO_VK
        .binary_search_by_key(&code, |(hid, _)| *hid)
        .ok()
        .map(|idx| u32::from(HID_TO_VK[idx].1 .0))
}

/// Win32 虚拟键码 → HID 用法 ID。
///
/// 表里没有的虚拟键码返回 `None`。
#[cfg(windows)]
pub fn vk_to_hid(vk: u32) -> Option<u16> {
    HID_TO_VK
        .iter()
        .find(|(_, v)| u32::from(v.0) == vk)
        .map(|(hid, _)| *hid)
}

/// 非 Windows 平台没有 Win32 虚拟键码 ⇒ 一律 `None`。
#[cfg(not(windows))]
pub fn hid_to_vk(_code: u16) -> Option<u32> {
    None
}

/// 非 Windows 平台没有 Win32 虚拟键码 ⇒ 一律 `None`。
#[cfg(not(windows))]
pub fn vk_to_hid(_vk: u32) -> Option<u16> {
    None
}

/// USB HID 用法 ID（Keyboard/Keypad page）→ Win32 `VIRTUAL_KEY`。
///
/// **必须按 HID 码严格升序**（`binary_search_by_key` 的前提），且不重复。
#[cfg(windows)]
pub const HID_TO_VK: &[(u16, VIRTUAL_KEY)] = &[
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
