package dev.beam.core.net

import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [SessionStatsTickSource] 的映射测试。
 *
 * 重点：**只搬运真实字段** —— 统计里没有的阶段不出现，取值为 0 的字段不写入。
 */
class SessionStatsTickSourceTest {

    @Test
    fun `真实统计被逐字段搬运`() = runTest {
        val stats = SessionStats(
            fps = 59.94,
            bitrateKbps = 18_500.0,
            rttMs = 8.5,
            lossPercent = 0.2,
            fecRecoveredPercent = 1.0,
            decodeMs = 3.2,
            renderMs = 1.4,
            e2eMs = 16.7,
            droppedFrames = 3L,
            width = 1920,
            height = 1080,
        )

        val tick = SessionStatsTickSource(flowOf(stats)).perfTicks().first()

        assertEquals(59.94, tick.fps, 1e-9)
        assertEquals(18_500.0, tick.bitrateKbps, 1e-9)
        assertEquals(3L, tick.droppedFrames)
        assertEquals(16.7, tick.e2eMs, 1e-9)
        assertEquals(3.2, tick.stagesMs[SessionStatsTickSource.STAGE_DECODE]!!, 1e-9)
        assertEquals(1.4, tick.stagesMs[SessionStatsTickSource.STAGE_RENDER]!!, 1e-9)
        assertEquals(8.5, tick.stagesMs[SessionStatsTickSource.STAGE_NETWORK]!!, 1e-9)
        assertEquals(16.7, tick.stagesMs[SessionStatsTickSource.STAGE_E2E]!!, 1e-9)
    }

    @Test
    fun `没有采样时不产生阶段`() = runTest {
        val tick = SessionStatsTickSource(flowOf(SessionStats())).perfTicks().first()

        // 0 代表「没有采样」，不能当成「耗时 0 ms」写进阶段表。
        assertTrue(tick.stagesMs.isEmpty())
        assertEquals(0.0, tick.fps, 1e-9)
        assertEquals(0L, tick.droppedFrames)
    }
}
