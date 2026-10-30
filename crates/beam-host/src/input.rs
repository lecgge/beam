//! 主机侧输入接线：权限映射 + 真实注入器（§3.9 主机编排 / §3.8 按设备权限）。
//!
//! ## 这一层存在的理由
//!
//! §3.8 的权限有两个来源，**分属两个 crate**：
//!
//! - 协议侧：`beam_pair::TrustedPeer.perms`（`PeerPerms{view,input,audio,gamepad}`）——
//!   **单设备闸门**：这一台对端到底准不准回传键鼠；
//! - 配置侧：`beam_config::InputSettings{gamepad, allow_keyboard, allow_mouse}`——
//!   **主机全局上限**：这台主机允不允许注入键盘 / 鼠标 / 手柄。
//!
//! §3.8 的边界要求 `beam-input` **不认识**这两个结构（它只依赖 proto / windows /
//! vigem-client），所以映射必须发生在**上层**——也就是本 crate。
//!
//! ## 映射规则（交集）
//!
//! | 设备 | 结果 |
//! |---|---|
//! | 键盘 | `对端权限.input` **且** `配置.allow_keyboard` |
//! | 鼠标 | `对端权限.input` **且** `配置.allow_mouse` |
//! | 手柄 | `对端权限.gamepad` **且** `配置.gamepad` |
//!
//! - 协议侧 `perms.input == false` ⇒ **键盘与鼠标都禁**（键鼠同属「回传输入」）；
//! - 协议侧 `perms.gamepad` 只管手柄，与键鼠无关；
//! - 两边取**交集**：配置是主机全局上限，对端权限是单设备闸门，**任一为假即禁**。
//!
//! ## 对端权限未知时
//!
//! [`map_input_permissions`] 的 `peer` 参数为 `None` 表示「这一层还不知道对端是谁」。
//! 此时**协议侧不施加限制**，只受配置侧约束 —— 因为能走到这一步的对端**已经过了
//! 信任闸门**（§3.3：不在信任库里的对端根本完不成握手）。
//!
//! 要启用「单设备闸门」，调用方必须把对端权限喂进来：
//! [`crate::runtime::HostRuntime::set_peer_permissions`]。
//! **这条边界要如实告诉调用方**：不喂对端权限 = 单设备闸门没生效。

use beam_config::InputSettings;
use beam_input::InputPermissions;
use beam_pair::PeerPerms;

/// 把「协议侧对端权限」×「配置侧输入设置」映射成按设备的注入权限。
///
/// 规则见模块文档的映射表：两边取**交集**。
///
/// `peer` 为 `None` 时协议侧不施加限制（对端身份未知，但已过信任闸门）。
///
/// # Examples
///
/// ```
/// use beam_config::InputSettings;
/// use beam_input::InputPermissions;
/// use beam_pair::PeerPerms;
/// use beam_host::map_input_permissions;
///
/// let cfg = InputSettings { gamepad: true, allow_keyboard: true, allow_mouse: false };
/// let peer = PeerPerms { view: true, input: true, audio: true, gamepad: true };
///
/// // 交集：鼠标被配置侧禁掉，手柄两侧都允许。
/// let perms = map_input_permissions(Some(&peer), &cfg);
/// assert!(perms.keyboard);
/// assert!(!perms.mouse);
/// assert!(perms.gamepad);
///
/// // 对端不允许回传输入 ⇒ 键鼠都禁。
/// let denied = PeerPerms { input: false, ..peer };
/// let perms = map_input_permissions(Some(&denied), &cfg);
/// assert!(!perms.keyboard);
/// assert!(!perms.mouse);
/// ```
pub fn map_input_permissions(peer: Option<&PeerPerms>, cfg: &InputSettings) -> InputPermissions {
    let (peer_input, peer_gamepad) = match peer {
        Some(perms) => (perms.input, perms.gamepad),
        None => (true, true),
    };
    InputPermissions {
        keyboard: peer_input && cfg.allow_keyboard,
        mouse: peer_input && cfg.allow_mouse,
        gamepad: peer_gamepad && cfg.gamepad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(gamepad: bool, allow_keyboard: bool, allow_mouse: bool) -> InputSettings {
        InputSettings {
            gamepad,
            allow_keyboard,
            allow_mouse,
        }
    }

    fn peer(input: bool, gamepad: bool) -> PeerPerms {
        PeerPerms {
            view: true,
            input,
            audio: true,
            gamepad,
        }
    }

    #[test]
    fn config_side_alone_gates_each_device() {
        let cfg = settings(true, true, false);
        let perms = map_input_permissions(None, &cfg);
        assert!(perms.keyboard, "配置允许键盘 → 键盘允许");
        assert!(!perms.mouse, "配置禁掉鼠标 → 鼠标禁");
        assert!(perms.gamepad, "配置启用手柄 → 手柄允许");
    }

    #[test]
    fn peer_input_false_disables_keyboard_and_mouse() {
        let cfg = settings(true, true, true);
        let perms = map_input_permissions(Some(&peer(false, true)), &cfg);
        assert!(!perms.keyboard, "对端不允许回传输入 → 键盘禁");
        assert!(!perms.mouse, "对端不允许回传输入 → 鼠标禁");
        assert!(perms.gamepad, "手柄由 perms.gamepad 单独决定，与键鼠无关");
    }

    #[test]
    fn peer_gamepad_false_disables_gamepad_only() {
        let cfg = settings(true, true, true);
        let perms = map_input_permissions(Some(&peer(true, false)), &cfg);
        assert!(perms.keyboard);
        assert!(perms.mouse);
        assert!(!perms.gamepad);
    }

    #[test]
    fn the_two_sides_are_intersected() {
        // 配置侧允许键鼠但禁手柄；对端允许手柄但禁键鼠 → 交集全禁。
        let cfg = settings(false, true, true);
        let perms = map_input_permissions(Some(&peer(false, true)), &cfg);
        assert!(!perms.keyboard);
        assert!(!perms.mouse);
        assert!(!perms.gamepad);

        // 配置侧禁键鼠、对端全允许 → 键鼠仍禁（配置是主机全局上限）。
        let cfg = settings(true, false, false);
        let perms = map_input_permissions(Some(&peer(true, true)), &cfg);
        assert!(!perms.keyboard);
        assert!(!perms.mouse);
        assert!(perms.gamepad);
    }

    #[test]
    fn unknown_peer_only_obeys_the_config_side() {
        let cfg = settings(false, false, false);
        let perms = map_input_permissions(None, &cfg);
        assert!(!perms.keyboard);
        assert!(!perms.mouse);
        assert!(!perms.gamepad);
    }
}
