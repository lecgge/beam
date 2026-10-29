package dev.beam.core.input

import dev.beam.core.net.GamepadAxis
import dev.beam.core.net.GamepadButton
import dev.beam.core.net.InputEvent
import dev.beam.core.net.TouchAction
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [InputMapper] 单元测试。
 *
 * 覆盖：触控归一化、Y 轴反转、摇杆死区、D-pad 映射、键鼠事件。
 */
class InputMapperTest {

    private val mapper = InputMapper()

    @Test
    fun `触控坐标按画面尺寸归一化`() {
        val event = mapper.touch(
            pointerId = 0,
            action = TouchAction.DOWN,
            x = 480f,
            y = 270f,
            viewWidth = 960f,
            viewHeight = 540f,
            timestampUs = 1_000L,
        )
        assertEquals(0.5f, event.normalizedX, 0.0001f)
        assertEquals(0.5f, event.normalizedY, 0.0001f)
        assertEquals(480f, event.x, 0.0001f)
        assertEquals(TouchAction.DOWN, event.action)
    }

    @Test
    fun `归一化坐标被限制在 0 到 1`() {
        val event = mapper.touch(
            pointerId = 0,
            action = TouchAction.MOVE,
            x = 1200f,
            y = -40f,
            viewWidth = 960f,
            viewHeight = 540f,
            timestampUs = 1_000L,
        )
        assertEquals(1f, event.normalizedX, 0.0001f)
        assertEquals(0f, event.normalizedY, 0.0001f)
    }

    @Test
    fun `开启 invertY 后 Y 轴翻转`() {
        val inverted = InputMapper(InputMapperConfig(invertY = true))
        val event = inverted.touch(
            pointerId = 0,
            action = TouchAction.DOWN,
            x = 0f,
            y = 540f,
            viewWidth = 960f,
            viewHeight = 540f,
            timestampUs = 1_000L,
        )
        assertEquals(0f, event.normalizedY, 0.0001f)
    }

    @Test
    fun `摇杆死区内不下发事件`() {
        assertNull(mapper.gamepadAxis(GamepadAxis.LEFT_X, 0.05f, 1_000L))
        assertNull(mapper.gamepadAxis(GamepadAxis.LEFT_X, -0.05f, 1_000L))
    }

    @Test
    fun `摇杆超出死区后线性重映射`() {
        val event = mapper.gamepadAxis(GamepadAxis.LEFT_X, 1f, 1_000L)
        assertNotNull(event)
        assertEquals(1f, event!!.value, 0.0001f)

        val half = mapper.gamepadAxis(GamepadAxis.LEFT_X, 0.54f, 1_000L)
        assertNotNull(half)
        assertEquals(0.5f, half!!.value, 0.01f)
    }

    @Test
    fun `D-pad 映射为方向键`() {
        val up = mapper.dpad(DpadDirection.UP, pressed = true, timestampUs = 1_000L)
        assertTrue(up is InputEvent.Key)
        assertEquals("ArrowUp", up.code)
        assertTrue(up.pressed)

        val center = mapper.dpad(DpadDirection.CENTER, pressed = true, timestampUs = 1_000L)
        assertEquals("Enter", center.code)
    }

    @Test
    fun `键鼠事件字段与协议模型对齐`() {
        val key = mapper.key("KeyA", pressed = true, timestampUs = 7L)
        assertEquals("KeyA", key.code)
        assertEquals(7L, key.timestampUs)

        val move = mapper.mouseMove(dx = 3f, dy = -4f, timestampUs = 8L)
        assertEquals(3f, move.dx, 0.0001f)
        assertEquals(-4f, move.dy, 0.0001f)

        val button = mapper.mouseButton(button = 1, pressed = true, timestampUs = 9L)
        assertEquals(1, button.button)

        val wheel = mapper.mouseWheel(deltaX = 0f, deltaY = 120f, timestampUs = 10L)
        assertEquals(120f, wheel.deltaY, 0.0001f)
    }

    @Test
    fun `手柄按键映射为协议事件`() {
        val event = mapper.gamepadButton(GamepadButton.A, pressed = true, timestampUs = 11L)
        assertEquals(GamepadButton.A, event.button)
        assertTrue(event.pressed)
    }
}
