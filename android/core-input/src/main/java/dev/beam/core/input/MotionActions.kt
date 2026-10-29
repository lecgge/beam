package dev.beam.core.input

import android.view.MotionEvent
import dev.beam.core.net.TouchAction

/**
 * Android 触控事件 → 协议触控动作的转换。
 *
 * 拆成独立对象是为了让映射逻辑可以在 JVM 单元测试里跑（`MotionEvent` 常量是
 * 纯数字，不需要真实设备）。
 */
object MotionActions {

    /**
     * 把 `MotionEvent` 的 action 映射成 [TouchAction]。
     *
     * @param action `MotionEvent.getActionMasked()` 的结果。
     * @return 无法识别的 action 返回 null。
     */
    fun toTouchAction(action: Int): TouchAction? = when (action) {
        MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> TouchAction.DOWN
        MotionEvent.ACTION_MOVE -> TouchAction.MOVE
        MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> TouchAction.UP
        MotionEvent.ACTION_CANCEL -> TouchAction.CANCEL
        else -> null
    }

    /**
     * 取触点 id。
     *
     * @param event 触控事件。
     * @param index 触点索引。
     */
    fun pointerId(event: MotionEvent, index: Int): Int =
        if (index in 0 until event.pointerCount) event.getPointerId(index) else 0

    /**
     * 取压力（0..1）；设备不支持时返回 1。
     */
    fun pressure(event: MotionEvent, index: Int): Float {
        if (index !in 0 until event.pointerCount) return 1f
        val p = event.getPressure(index)
        return if (p > 0f) p.coerceIn(0f, 1f) else 1f
    }
}
