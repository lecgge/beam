package dev.beam.core.input

import android.view.KeyEvent
import android.view.MotionEvent
import dev.beam.core.net.GamepadAxis
import dev.beam.core.net.GamepadButton
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Android 按键 / 手柄轴映射的单测。
 *
 * `android.view.KeyEvent` / `MotionEvent` 的**常量**是编译期内联的，
 * 因此这些映射可以在 JVM 单测里直接验证（不需要真机）。
 */
class AndroidKeysTest {

    @Test
    fun `方向键映射成协议方向码`() {
        assertEquals("ArrowUp", AndroidKeys.protocolCode(KeyEvent.KEYCODE_DPAD_UP))
        assertEquals("ArrowDown", AndroidKeys.protocolCode(KeyEvent.KEYCODE_DPAD_DOWN))
        assertEquals("ArrowLeft", AndroidKeys.protocolCode(KeyEvent.KEYCODE_DPAD_LEFT))
        assertEquals("ArrowRight", AndroidKeys.protocolCode(KeyEvent.KEYCODE_DPAD_RIGHT))
    }

    @Test
    fun `确认键映射成 Enter 且与遥控器中心键一致`() {
        assertEquals("Enter", AndroidKeys.protocolCode(KeyEvent.KEYCODE_ENTER))
        assertEquals("Enter", AndroidKeys.protocolCode(KeyEvent.KEYCODE_DPAD_CENTER))
        assertEquals("Enter", AndroidKeys.protocolCode(KeyEvent.KEYCODE_NUMPAD_ENTER))
    }

    @Test
    fun `字母与数字键有稳定的协议码`() {
        assertEquals("KeyA", AndroidKeys.protocolCode(KeyEvent.KEYCODE_A))
        assertEquals("KeyZ", AndroidKeys.protocolCode(KeyEvent.KEYCODE_Z))
        assertEquals("Digit0", AndroidKeys.protocolCode(KeyEvent.KEYCODE_0))
        assertEquals("Digit9", AndroidKeys.protocolCode(KeyEvent.KEYCODE_9))
        assertEquals("F1", AndroidKeys.protocolCode(KeyEvent.KEYCODE_F1))
    }

    @Test
    fun `无法识别的按键返回 null`() {
        assertNull(AndroidKeys.protocolCode(KeyEvent.KEYCODE_UNKNOWN))
        assertNull(AndroidKeys.protocolCode(0xFFFF))
    }

    @Test
    fun `手柄按键映射到手柄枚举`() {
        assertEquals(GamepadButton.A, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_BUTTON_A))
        assertEquals(GamepadButton.B, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_BUTTON_B))
        assertEquals(GamepadButton.X, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_BUTTON_X))
        assertEquals(GamepadButton.Y, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_BUTTON_Y))
        assertEquals(GamepadButton.START, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_BUTTON_START))
        assertEquals(GamepadButton.BACK, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_BUTTON_SELECT))
        assertEquals(GamepadButton.DPAD_UP, AndroidKeys.gamepadButton(KeyEvent.KEYCODE_DPAD_UP))
    }

    @Test
    fun `普通按键不是手柄按键`() {
        assertNull(AndroidKeys.gamepadButton(KeyEvent.KEYCODE_A))
        assertNull(AndroidKeys.gamepadButton(KeyEvent.KEYCODE_ENTER))
    }

    @Test
    fun `Android 轴索引映射到协议轴`() {
        assertEquals(GamepadAxis.LEFT_X, GamepadMotion.axisFor(MotionEvent.AXIS_X))
        assertEquals(GamepadAxis.LEFT_Y, GamepadMotion.axisFor(MotionEvent.AXIS_Y))
        assertEquals(GamepadAxis.RIGHT_X, GamepadMotion.axisFor(MotionEvent.AXIS_Z))
        assertEquals(GamepadAxis.RIGHT_Y, GamepadMotion.axisFor(MotionEvent.AXIS_RZ))
        assertEquals(GamepadAxis.TRIGGER_LEFT, GamepadMotion.axisFor(MotionEvent.AXIS_LTRIGGER))
        assertEquals(GamepadAxis.TRIGGER_RIGHT, GamepadMotion.axisFor(MotionEvent.AXIS_RTRIGGER))
        assertNull(GamepadMotion.axisFor(MotionEvent.AXIS_PRESSURE))
    }

    @Test
    fun `摇杆死区内的轴不下发事件`() {
        val mapper = InputMapper(InputMapperConfig(stickDeadzone = 0.2f))
        assertNull(mapper.gamepadAxis(GamepadAxis.LEFT_X, 0.1f, 0L))
        assertNotNull(mapper.gamepadAxis(GamepadAxis.LEFT_X, 0.9f, 0L))
    }
}
