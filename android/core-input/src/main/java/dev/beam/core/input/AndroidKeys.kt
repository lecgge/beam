package dev.beam.core.input

import android.view.KeyEvent
import dev.beam.core.net.GamepadButton

/**
 * Android 按键 → 协议按键码的映射。
 *
 * ## 协议按键码约定
 *
 * 协议侧（`beam-proto::InputEvent::Key`）用的是 USB HID 扫描码（`u16`），
 * Android 侧统一用**可读的 HID 用法名**（`KeyA` / `ArrowUp` / `Enter` …），
 * 由主机端翻译成扫描码。遥控器方向键沿用 [DpadDirection] 已有的取值。
 *
 * ## 禁止事项
 * 本对象只做**按键映射**，不触碰网络，也不直接发事件（发事件由 [InputMapper] 负责）。
 */
object AndroidKeys {

    /**
     * 把 Android `KeyEvent.keyCode` 映射成协议按键码。
     *
     * @return 无法识别的按键返回 null（调用方应当忽略该按键）。
     */
    fun protocolCode(keyCode: Int): String? = when (keyCode) {
        // --- 方向键 / 遥控器 ---
        KeyEvent.KEYCODE_DPAD_UP -> "ArrowUp"
        KeyEvent.KEYCODE_DPAD_DOWN -> "ArrowDown"
        KeyEvent.KEYCODE_DPAD_LEFT -> "ArrowLeft"
        KeyEvent.KEYCODE_DPAD_RIGHT -> "ArrowRight"
        KeyEvent.KEYCODE_DPAD_CENTER -> "Enter"

        // --- 确认 / 返回 / 系统 ---
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> "Enter"
        KeyEvent.KEYCODE_ESCAPE -> "Escape"
        KeyEvent.KEYCODE_DEL -> "Backspace"
        KeyEvent.KEYCODE_FORWARD_DEL -> "Delete"
        KeyEvent.KEYCODE_TAB -> "Tab"
        KeyEvent.KEYCODE_SPACE -> "Space"
        KeyEvent.KEYCODE_MENU -> "ContextMenu"
        KeyEvent.KEYCODE_BACK -> "BrowserBack"
        KeyEvent.KEYCODE_HOME -> "BrowserHome"

        // --- 字母 ---
        KeyEvent.KEYCODE_A -> "KeyA"
        KeyEvent.KEYCODE_B -> "KeyB"
        KeyEvent.KEYCODE_C -> "KeyC"
        KeyEvent.KEYCODE_D -> "KeyD"
        KeyEvent.KEYCODE_E -> "KeyE"
        KeyEvent.KEYCODE_F -> "KeyF"
        KeyEvent.KEYCODE_G -> "KeyG"
        KeyEvent.KEYCODE_H -> "KeyH"
        KeyEvent.KEYCODE_I -> "KeyI"
        KeyEvent.KEYCODE_J -> "KeyJ"
        KeyEvent.KEYCODE_K -> "KeyK"
        KeyEvent.KEYCODE_L -> "KeyL"
        KeyEvent.KEYCODE_M -> "KeyM"
        KeyEvent.KEYCODE_N -> "KeyN"
        KeyEvent.KEYCODE_O -> "KeyO"
        KeyEvent.KEYCODE_P -> "KeyP"
        KeyEvent.KEYCODE_Q -> "KeyQ"
        KeyEvent.KEYCODE_R -> "KeyR"
        KeyEvent.KEYCODE_S -> "KeyS"
        KeyEvent.KEYCODE_T -> "KeyT"
        KeyEvent.KEYCODE_U -> "KeyU"
        KeyEvent.KEYCODE_V -> "KeyV"
        KeyEvent.KEYCODE_W -> "KeyW"
        KeyEvent.KEYCODE_X -> "KeyX"
        KeyEvent.KEYCODE_Y -> "KeyY"
        KeyEvent.KEYCODE_Z -> "KeyZ"

        // --- 数字 ---
        KeyEvent.KEYCODE_0 -> "Digit0"
        KeyEvent.KEYCODE_1 -> "Digit1"
        KeyEvent.KEYCODE_2 -> "Digit2"
        KeyEvent.KEYCODE_3 -> "Digit3"
        KeyEvent.KEYCODE_4 -> "Digit4"
        KeyEvent.KEYCODE_5 -> "Digit5"
        KeyEvent.KEYCODE_6 -> "Digit6"
        KeyEvent.KEYCODE_7 -> "Digit7"
        KeyEvent.KEYCODE_8 -> "Digit8"
        KeyEvent.KEYCODE_9 -> "Digit9"

        // --- 功能键 ---
        KeyEvent.KEYCODE_F1 -> "F1"
        KeyEvent.KEYCODE_F2 -> "F2"
        KeyEvent.KEYCODE_F3 -> "F3"
        KeyEvent.KEYCODE_F4 -> "F4"
        KeyEvent.KEYCODE_F5 -> "F5"
        KeyEvent.KEYCODE_F6 -> "F6"
        KeyEvent.KEYCODE_F7 -> "F7"
        KeyEvent.KEYCODE_F8 -> "F8"
        KeyEvent.KEYCODE_F9 -> "F9"
        KeyEvent.KEYCODE_F10 -> "F10"
        KeyEvent.KEYCODE_F11 -> "F11"
        KeyEvent.KEYCODE_F12 -> "F12"

        else -> null
    }

    /**
     * 把 Android `KeyEvent.keyCode` 映射成手柄按键。
     *
     * 手柄按键在 Android 上以 `KEYCODE_BUTTON_*` 的形式上报为**按键事件**，
     * 与模拟轴（摇杆 / 扳机）走的是两条不同的通道。
     *
     * @return 不是手柄按键时返回 null。
     */
    fun gamepadButton(keyCode: Int): GamepadButton? = when (keyCode) {
        KeyEvent.KEYCODE_BUTTON_A -> GamepadButton.A
        KeyEvent.KEYCODE_BUTTON_B -> GamepadButton.B
        KeyEvent.KEYCODE_BUTTON_X -> GamepadButton.X
        KeyEvent.KEYCODE_BUTTON_Y -> GamepadButton.Y
        KeyEvent.KEYCODE_DPAD_UP -> GamepadButton.DPAD_UP
        KeyEvent.KEYCODE_DPAD_DOWN -> GamepadButton.DPAD_DOWN
        KeyEvent.KEYCODE_DPAD_LEFT -> GamepadButton.DPAD_LEFT
        KeyEvent.KEYCODE_DPAD_RIGHT -> GamepadButton.DPAD_RIGHT
        KeyEvent.KEYCODE_BUTTON_L1 -> GamepadButton.LEFT_SHOULDER
        KeyEvent.KEYCODE_BUTTON_R1 -> GamepadButton.RIGHT_SHOULDER
        KeyEvent.KEYCODE_BUTTON_THUMBL -> GamepadButton.LEFT_STICK
        KeyEvent.KEYCODE_BUTTON_THUMBR -> GamepadButton.RIGHT_STICK
        KeyEvent.KEYCODE_BUTTON_START -> GamepadButton.START
        KeyEvent.KEYCODE_BUTTON_SELECT -> GamepadButton.BACK
        else -> null
    }
}
