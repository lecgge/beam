package dev.beam.core.data.ui

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Toast 控制器行为测试（短提示 + 自动消失）。
 *
 * 注意：`advanceTimeBy` **不会**执行恰好落在目标时刻上的任务，所以每次都多推进 1 ms。
 */
@OptIn(ExperimentalCoroutinesApi::class)
class BeamToastControllerTest {

    @Test
    fun `提示在指定时长后自动消失`() = runTest {
        val controller = BeamToastController(backgroundScope, durationMs = 2_000L)

        controller.show("已复制")
        assertEquals("已复制", controller.message.value)

        advanceTimeBy(1_999L)
        assertEquals("已复制", controller.message.value)

        advanceTimeBy(2L)
        assertNull(controller.message.value)
    }

    @Test
    fun `新的提示覆盖旧提示并重新计时`() = runTest {
        val controller = BeamToastController(backgroundScope, durationMs = 2_000L)

        controller.show("已复制")
        advanceTimeBy(1_500L)
        controller.show("已保存")
        assertEquals("已保存", controller.message.value)

        // 旧计时器已经取消，不会提前把新提示收走。
        advanceTimeBy(1_000L)
        assertEquals("已保存", controller.message.value)

        // 新提示的计时从覆盖那一刻起算：1500 + 2000 = 3500。
        advanceTimeBy(1_001L)
        assertNull(controller.message.value)
    }

    @Test
    fun `dismiss 立即收起提示`() = runTest {
        val controller = BeamToastController(backgroundScope, durationMs = 2_000L)

        controller.show("已复制")
        controller.dismiss()
        assertNull(controller.message.value)
    }
}
