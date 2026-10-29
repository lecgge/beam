package dev.beam.core.input

import dev.beam.core.net.GamepadAxis
import dev.beam.core.net.GamepadButton
import dev.beam.core.net.InputEvent
import dev.beam.core.net.TouchAction

/**
 * 遥控器 / D-pad 方向。
 *
 * Android TV 与遥控器没有自由指针，只能映射成方向键（§3.12 `:core-input`）。
 */
enum class DpadDirection(val keyCode: String) {
    UP("ArrowUp"),
    DOWN("ArrowDown"),
    LEFT("ArrowLeft"),
    RIGHT("ArrowRight"),
    CENTER("Enter"),
    ;
}

/**
 * 映射配置。
 */
data class InputMapperConfig(
    /** 摇杆死区（0..1），低于该值视为 0。 */
    val stickDeadzone: Float = 0.08f,
    /** 触控坐标是否按画面尺寸归一化。 */
    val normalizeTouch: Boolean = true,
    /** 是否上下反转 Y（部分主机坐标系原点在左下）。 */
    val invertY: Boolean = false,
)

/**
 * 输入映射器（§3.12 `:core-input` 的对外接口）。
 *
 * 把触控 / 实体手柄 / 遥控器映射成 `InputEvent`。
 *
 * **字段与 `:core-net` 的 `InputEvent.kt` 完全对齐** —— `:core-net` 负责线上协议
 * 模型，本类只做「设备事件 → 协议事件」的映射，不定义新的事件类型。
 *
 * ## 线程模型
 * 无状态（除配置），可在任意线程调用；建议在主线程或输入回调线程调用。
 */
class InputMapper(
    private val config: InputMapperConfig = InputMapperConfig(),
) {

    /**
     * 映射触控事件。
     *
     * @param pointerId 触点 id。
     * @param action    触控动作。
     * @param x         相对视图的 X（像素）。
     * @param y         相对视图的 Y（像素）。
     * @param viewWidth 视图宽度（像素），用于归一化。
     * @param viewHeight 视图高度（像素），用于归一化。
     * @param pressure  压力（0..1）。
     * @param timestampUs 单调时钟微秒。
     */
    fun touch(
        pointerId: Int,
        action: TouchAction,
        x: Float,
        y: Float,
        viewWidth: Float,
        viewHeight: Float,
        pressure: Float = 1f,
        timestampUs: Long,
    ): InputEvent.Touch {
        val nx = if (config.normalizeTouch && viewWidth > 0f) clamp01(x / viewWidth) else x
        val nyRaw = if (config.normalizeTouch && viewHeight > 0f) clamp01(y / viewHeight) else y
        val ny = if (config.invertY) 1f - nyRaw else nyRaw
        return InputEvent.Touch(
            timestampUs = timestampUs,
            pointerId = pointerId,
            action = action,
            x = x,
            y = y,
            normalizedX = nx,
            normalizedY = ny,
            pressure = pressure,
        )
    }

    /** 映射按键（物理键盘 / 遥控器方向键）。 */
    fun key(code: String, pressed: Boolean, timestampUs: Long, repeat: Boolean = false): InputEvent.Key =
        InputEvent.Key(timestampUs = timestampUs, code = code, pressed = pressed, repeat = repeat)

    /** 映射 D-pad / 遥控器方向。 */
    fun dpad(direction: DpadDirection, pressed: Boolean, timestampUs: Long): InputEvent.Key =
        key(direction.keyCode, pressed, timestampUs)

    /** 映射鼠标移动。 */
    fun mouseMove(dx: Float, dy: Float, timestampUs: Long): InputEvent.MouseMove =
        InputEvent.MouseMove(timestampUs = timestampUs, dx = dx, dy = dy)

    /** 映射鼠标按键。 */
    fun mouseButton(button: Int, pressed: Boolean, timestampUs: Long): InputEvent.MouseButton =
        InputEvent.MouseButton(timestampUs = timestampUs, button = button, pressed = pressed)

    /** 映射滚轮。 */
    fun mouseWheel(deltaX: Float, deltaY: Float, timestampUs: Long): InputEvent.MouseWheel =
        InputEvent.MouseWheel(timestampUs = timestampUs, deltaX = deltaX, deltaY = deltaY)

    /** 映射手柄按键。 */
    fun gamepadButton(button: GamepadButton, pressed: Boolean, timestampUs: Long): InputEvent.GamepadButton =
        InputEvent.GamepadButton(timestampUs = timestampUs, button = button, pressed = pressed)

    /**
     * 映射手柄摇杆 / 扳机。
     *
     * 死区内的值返回 null（不下发事件），避免网络上一堆无效输入。
     */
    fun gamepadAxis(axis: GamepadAxis, value: Float, timestampUs: Long): InputEvent.GamepadAxis? {
        val applied = applyDeadzone(value)
        if (applied == 0f) return null
        return InputEvent.GamepadAxis(timestampUs = timestampUs, axis = axis, value = applied)
    }

    /** 摇杆死区处理：超出死区后线性重映射到 [-1, 1] / [0, 1]。 */
    private fun applyDeadzone(value: Float): Float {
        val magnitude = kotlin.math.abs(value)
        if (magnitude <= config.stickDeadzone) return 0f
        val sign = if (value < 0f) -1f else 1f
        val scaled = (magnitude - config.stickDeadzone) / (1f - config.stickDeadzone)
        return sign * scaled.coerceIn(0f, 1f)
    }

    private fun clamp01(value: Float): Float = value.coerceIn(0f, 1f)
}
