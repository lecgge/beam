package dev.beam.core.net

/**
 * 输入事件模型。
 *
 * **字段与 `beam-proto::InputEvent` 对齐**，是客户端 → 主机唯一的输入载体。
 * 放在 `:core-net` 而不是 `:core-input`，是因为它属于线上协议模型：
 * `:core-net` 负责序列化下发，`:core-input` 只负责把设备事件映射成本模型
 * （见 `:core-input` 的 README）。
 */
sealed interface InputEvent {

    /** 事件时间戳：单调时钟微秒。 */
    val timestampUs: Long

    /** 键鼠按键。 */
    data class Key(
        override val timestampUs: Long,
        val code: String,
        val pressed: Boolean,
        val repeat: Boolean = false,
    ) : InputEvent

    /** 指针移动（相对位移，像素）。 */
    data class MouseMove(
        override val timestampUs: Long,
        val dx: Float,
        val dy: Float,
    ) : InputEvent

    /** 鼠标按键。 */
    data class MouseButton(
        override val timestampUs: Long,
        val button: Int,
        val pressed: Boolean,
    ) : InputEvent

    /** 滚轮。 */
    data class MouseWheel(
        override val timestampUs: Long,
        val deltaX: Float,
        val deltaY: Float,
    ) : InputEvent

    /** 触摸（Android 端的主输入通道）。 */
    data class Touch(
        override val timestampUs: Long,
        val pointerId: Int,
        val action: TouchAction,
        val x: Float,
        val y: Float,
        /** 归一化到画面宽高的坐标，主机侧据此换算回绝对坐标。 */
        val normalizedX: Float,
        val normalizedY: Float,
        val pressure: Float = 1f,
    ) : InputEvent

    /** 手柄按键。 */
    data class GamepadButton(
        override val timestampUs: Long,
        // 全限定名：避免解析到本文件内同名的嵌套数据类 GamepadButton。
        val button: dev.beam.core.net.GamepadButton,
        val pressed: Boolean,
    ) : InputEvent

    /** 手柄摇杆。 */
    data class GamepadAxis(
        override val timestampUs: Long,
        // 全限定名：避免解析到本文件内同名的嵌套数据类 GamepadAxis。
        val axis: dev.beam.core.net.GamepadAxis,
        val value: Float,
    ) : InputEvent
}

/** 触摸动作。 */
enum class TouchAction { DOWN, MOVE, UP, CANCEL }

/** 手柄按键（Xbox 布局）。 */
enum class GamepadButton {
    A, B, X, Y,
    DPAD_UP, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT,
    LEFT_SHOULDER, RIGHT_SHOULDER,
    LEFT_STICK, RIGHT_STICK,
    START, BACK,
}

/** 手柄摇杆。 */
enum class GamepadAxis { LEFT_X, LEFT_Y, RIGHT_X, RIGHT_Y, TRIGGER_LEFT, TRIGGER_RIGHT }
