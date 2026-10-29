package dev.beam.core.renderer

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [FramePacer] 单元测试。
 *
 * 覆盖：首帧上屏、按时上屏、迟到丢帧、提前等待、乱序跳过、重置。
 */
class FramePacerTest {

    @Test
    fun `首帧直接上屏`() {
        val pacer = FramePacer(targetFps = 60)
        val decision = pacer.decide(ptsUs = 1_000_000L, arriveUs = 1_000_000L)
        assertTrue(decision is PacerDecision.Present)
    }

    @Test
    fun `按时到达上屏`() {
        val pacer = FramePacer(targetFps = 60)
        pacer.decide(ptsUs = 0L, arriveUs = 0L)
        // 第二帧正好落后一个帧间隔（16.67 ms），仍在阈值内 → 上屏
        val decision = pacer.decide(ptsUs = 16_667L, arriveUs = 16_667L)
        assertTrue(decision is PacerDecision.Present)
    }

    @Test
    fun `迟到超过一个帧间隔则丢帧`() {
        val pacer = FramePacer(targetFps = 60)
        pacer.decide(ptsUs = 0L, arriveUs = 0L)
        val decision = pacer.decide(ptsUs = 100_000L, arriveUs = 100_000L)
        assertTrue(decision is PacerDecision.Drop)
    }

    @Test
    fun `提前到达则等待`() {
        val pacer = FramePacer(targetFps = 60)
        // 首帧在 0 到达 → 下一个节拍在 16 666 µs
        pacer.decide(ptsUs = 0L, arriveUs = 0L)
        // 第二帧在 11 667 µs 到达，比节拍早 4 999 µs
        val decision = pacer.decide(ptsUs = 11_667L, arriveUs = 11_667L)
        assertTrue(decision is PacerDecision.Wait)
        assertEquals(4_999L, (decision as PacerDecision.Wait).dueInUs)
    }

    @Test
    fun `pts 非递增则跳过`() {
        val pacer = FramePacer(targetFps = 60)
        pacer.decide(ptsUs = 10_000L, arriveUs = 10_000L)
        val decision = pacer.decide(ptsUs = 5_000L, arriveUs = 11_000L)
        assertTrue(decision is PacerDecision.Skip)
    }

    @Test
    fun `重复 pts 也跳过`() {
        val pacer = FramePacer(targetFps = 60)
        pacer.decide(ptsUs = 10_000L, arriveUs = 10_000L)
        val decision = pacer.decide(ptsUs = 10_000L, arriveUs = 11_000L)
        assertTrue(decision is PacerDecision.Skip)
    }

    @Test
    fun `reset 后重新以首帧为基准`() {
        val pacer = FramePacer(targetFps = 60)
        pacer.decide(ptsUs = 0L, arriveUs = 0L)
        pacer.reset()
        val decision = pacer.decide(ptsUs = 999_000L, arriveUs = 999_000L)
        assertTrue(decision is PacerDecision.Present)
    }

    @Test
    fun `帧间隔与目标帧率一致`() {
        assertEquals(16_666L, FramePacer(targetFps = 60).frameIntervalUs)
        assertEquals(8_333L, FramePacer(targetFps = 120).frameIntervalUs)
    }
}
