package dev.beam.core.input

import android.view.View
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.KeyEvent
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.changedToDownIgnoreConsumed
import androidx.compose.ui.input.pointer.changedToUpIgnoreConsumed
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChangedIgnoreConsumed
import androidx.compose.ui.platform.LocalView
import dev.beam.core.net.InputEvent
import dev.beam.core.net.TouchAction

/**
 * Compose 侧的输入采集 Modifier。
 *
 * ## 职责边界（§3.12）
 *
 * `:core-input` 只做**设备事件 → 协议事件**的映射：
 * - 本文件负责把 Compose 的触控 / 按键 / 手柄事件喂给 [InputMapper]；
 * - 产出的 [InputEvent] 由调用方（`:feature-stream`）交给 `:core-net` 回传主机。
 *
 * 本文件**不接触网络**，也不知道 Rust 侧的存在。
 *
 * ## 线程模型
 * 触控与按键回调发生在主线程（Compose 的输入分发在主线程），
 * 因此 [onEvent] 里不要做阻塞操作。
 */

/**
 * 采集触控事件并映射成 [InputEvent.Touch]。
 *
 * 坐标按所在容器的尺寸归一化（[InputMapperConfig.normalizeTouch]），
 * 主机侧据此换算回绝对坐标。
 *
 * @param mapper 映射器。
 * @param onEvent 事件出口（通常是 `:core-net` 的会话发送）。
 */
fun Modifier.beamTouchInput(
    mapper: InputMapper = InputMapper(),
    onEvent: (InputEvent) -> Unit,
): Modifier = pointerInput(mapper, onEvent) {
    val viewWidth = size.width.toFloat()
    val viewHeight = size.height.toFloat()
    awaitPointerEventScope {
        while (true) {
            val event = awaitPointerEvent()
            for (change in event.changes) {
                val action = change.toTouchAction() ?: continue
                val pressure = change.pressure.takeIf { it > 0f } ?: 1f
                onEvent(
                    mapper.touch(
                        pointerId = change.id.value.toInt(),
                        action = action,
                        x = change.position.x,
                        y = change.position.y,
                        viewWidth = viewWidth,
                        viewHeight = viewHeight,
                        pressure = pressure,
                        timestampUs = InputClock.nowUs(),
                    ),
                )
            }
        }
    }
}

/**
 * 采集按键 / 遥控器 / 手柄按键并映射成 [InputEvent.Key] 或 [InputEvent.GamepadButton]。
 *
 * 事件**不消费**（返回 `false`），这样系统的返回键与无障碍行为不受影响；
 * 事件已经复制给主机，本地再处理一遍是合理的。
 *
 * @param mapper 映射器。
 * @param onEvent 事件出口。
 */
fun Modifier.beamKeyInput(
    mapper: InputMapper = InputMapper(),
    onEvent: (InputEvent) -> Unit,
): Modifier = this.onKeyEvent { event ->
    val pressed = when (event.type) {
        KeyEventType.KeyDown -> true
        KeyEventType.KeyUp -> false
        else -> return@onKeyEvent false
    }
    val native = event.nativeKeyEvent
    val timestampUs = InputClock.nowUs()

    // 手柄按键优先：它们在 Android 上也是按键事件，但语义是手柄按钮。
    val gamepad = AndroidKeys.gamepadButton(native.keyCode)
    if (gamepad != null) {
        onEvent(mapper.gamepadButton(gamepad, pressed, timestampUs))
        return@onKeyEvent false
    }

    val code = AndroidKeys.protocolCode(native.keyCode) ?: return@onKeyEvent false
    onEvent(
        mapper.key(
            code = code,
            pressed = pressed,
            timestampUs = timestampUs,
            repeat = pressed && native.repeatCount > 0,
        ),
    )
    false
}

/**
 * 采集手柄模拟轴（摇杆 / 扳机）。
 *
 * 模拟轴不走 Compose 的按键/指针通道，而是 Android 的 generic motion 通道，
 * 因此需要在宿主 `View` 上挂 [View.OnGenericMotionListener]。
 *
 * > 注意：同一个 `View` 只能挂一个 generic motion 监听器，
 * > 因此本 Composable 应当在串流页这类**独占**界面里使用。
 *
 * @param mapper 映射器。
 * @param onEvent 事件出口。
 */
@Composable
fun BeamGamepadInput(
    mapper: InputMapper = InputMapper(),
    onEvent: (InputEvent) -> Unit,
) {
    val view = LocalView.current
    val currentOnEvent by rememberUpdatedState(onEvent)
    DisposableEffect(view, mapper) {
        val listener = View.OnGenericMotionListener { _, event ->
            GamepadMotion.axisEvents(event, mapper).forEach { currentOnEvent(it) }
            true
        }
        view.setOnGenericMotionListener(listener)
        onDispose { view.setOnGenericMotionListener(null) }
    }
}

/** 把一次指针变化映射成触控动作；没有语义变化的指针返回 null。 */
private fun PointerInputChange.toTouchAction(): TouchAction? = when {
    changedToDownIgnoreConsumed() -> TouchAction.DOWN
    changedToUpIgnoreConsumed() -> TouchAction.UP
    positionChangedIgnoreConsumed() -> TouchAction.MOVE
    !pressed -> TouchAction.CANCEL
    else -> null
}
