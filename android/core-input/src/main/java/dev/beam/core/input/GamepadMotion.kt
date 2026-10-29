package dev.beam.core.input

import android.view.InputDevice
import android.view.MotionEvent
import dev.beam.core.net.GamepadAxis
import dev.beam.core.net.InputEvent

/**
 * 手柄模拟轴（摇杆 / 扳机）的采集与映射。
 *
 * ## 为什么单独一个对象
 *
 * 手柄的**按键**在 Android 上以 `KEYCODE_BUTTON_*` 的形式作为按键事件上报
 * （见 [AndroidKeys.gamepadButton]），而**模拟轴**走的是 generic motion 通道
 * （`MotionEvent` + `AXIS_*`）。两者通道不同，因此分开处理。
 *
 * ## 坐标系
 *
 * Android 的摇杆轴值是 `[-1, 1]`，但 **Y 轴向下为正**；
 * 协议侧（`beam-proto::InputEvent::GamepadAxis`）用 `Q15` 定点数，
 * 语义与 [InputMapper.gamepadAxis] 保持一致：**向上为正**，因此这里对 Y 取反。
 */
object GamepadMotion {

    /**
     * 从一次 generic motion 事件里解析出全部轴事件。
     *
     * @param event 原始 `MotionEvent`。
     * @param mapper 映射器（负责死区处理）。
     * @return 死区内的轴不会产出事件（列表可能为空）。
     */
    fun axisEvents(event: MotionEvent, mapper: InputMapper): List<InputEvent.GamepadAxis> {
        if (!isGamepadSource(event)) return emptyList()
        val timestampUs = InputClock.nowUs()
        val out = ArrayList<InputEvent.GamepadAxis>(4)
        for ((axisIndex, axis) in AXIS_MAP) {
            val raw = event.getAxisValue(axisIndex)
            // 轴没有变化时不上报（避免网络上一堆重复的 0 值）。
            if (raw == 0f) continue
            val value = if (axis == GamepadAxis.LEFT_Y || axis == GamepadAxis.RIGHT_Y) -raw else raw
            mapper.gamepadAxis(axis, value, timestampUs)?.let { out.add(it) }
        }
        return out
    }

    /** 事件来源是否是手柄 / 摇杆。 */
    fun isGamepadSource(event: MotionEvent): Boolean =
        event.isFromSource(InputDevice.SOURCE_JOYSTICK) ||
            event.isFromSource(InputDevice.SOURCE_GAMEPAD) ||
            event.isFromSource(InputDevice.SOURCE_DPAD)

    /**
     * Android 轴索引 → 协议轴。
     *
     * 单独抽出来是为了能直接单测（`MotionEvent` 在 JVM 上构造不出来）。
     */
    fun axisFor(axisIndex: Int): GamepadAxis? = AXIS_MAP[axisIndex]

    /** Android 轴索引 → 协议轴的映射表。 */
    private val AXIS_MAP: Map<Int, GamepadAxis> = mapOf(
        MotionEvent.AXIS_X to GamepadAxis.LEFT_X,
        MotionEvent.AXIS_Y to GamepadAxis.LEFT_Y,
        MotionEvent.AXIS_Z to GamepadAxis.RIGHT_X,
        MotionEvent.AXIS_RZ to GamepadAxis.RIGHT_Y,
        MotionEvent.AXIS_LTRIGGER to GamepadAxis.TRIGGER_LEFT,
        MotionEvent.AXIS_RTRIGGER to GamepadAxis.TRIGGER_RIGHT,
    )
}
