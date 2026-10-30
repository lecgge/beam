package dev.beam.core.net

import dev.beam.core.net.fakes.DiscoveredHost as GenHost
import dev.beam.core.net.fakes.Peer as GenPeer
import dev.beam.core.net.fakes.SessionEvent as GenEvent
import dev.beam.core.net.fakes.SessionStats as GenStats
import dev.beam.core.net.fakes.StreamCfg as GenCfg
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [NativeMappings] 的单测。
 *
 * 替身类型见 `dev.beam.core.net.fakes`（与 uniffi 生成物同构：
 * 变体名一致、统计字段在嵌套记录上、数值字段是无符号整型）。
 */
class NativeMappingsTest {

    // --------------------------------------------------------------- 统计

    @Test
    fun `stats 变体的统计字段在嵌套记录上`() {
        val raw = GenEvent.Stats(
            GenStats(
                sessionId = 42uL,
                fps = 60u,
                bitrateBps = 20_000_000u,
                lossPpm = 1_500u,
                rttUs = 8_000u,
                decodeUs = 3_000u,
                renderUs = 1_500u,
                e2eUs = 12_500u,
                droppedFrames = 7u,
            ),
        )

        val mapped = NativeMappings.mapStats(raw)

        // 帧率是整数比例，直接取。
        assertEquals(60.0, mapped.fps, 1e-9)
        // bit/s → kbit/s
        assertEquals(20_000.0, mapped.bitrateKbps, 1e-6)
        // µs → ms
        assertEquals(8.0, mapped.rttMs, 1e-9)
        assertEquals(3.0, mapped.decodeMs, 1e-9)
        assertEquals(1.5, mapped.renderMs, 1e-9)
        assertEquals(12.5, mapped.e2eMs, 1e-9)
        // ppm → 百分比
        assertEquals(0.15, mapped.lossPercent, 1e-9)
        assertEquals(7L, mapped.droppedFrames)
    }

    @Test
    fun `统计事件映射后字段不为零`() {
        val raw = GenEvent.Stats(
            GenStats(
                sessionId = 1uL,
                fps = 58u,
                bitrateBps = 18_500_000u,
                lossPpm = 2_000u,
                rttUs = 9_000u,
                decodeUs = 4_000u,
                renderUs = 2_000u,
                e2eUs = 15_000u,
                droppedFrames = 3u,
            ),
        )

        val event = NativeMappings.mapSessionEvent(raw)

        assertTrue("统计事件应被识别", event is SessionEvent.Stats)
        val stats = (event as SessionEvent.Stats).stats
        assertTrue("帧率不应为 0", stats.fps > 0.0)
        assertTrue("码率不应为 0", stats.bitrateKbps > 0.0)
        assertTrue("RTT 不应为 0", stats.rttMs > 0.0)
        assertTrue("丢包率不应为 0", stats.lossPercent > 0.0)
        assertTrue("解码耗时不应为 0", stats.decodeMs > 0.0)
        assertTrue("上屏耗时不应为 0", stats.renderMs > 0.0)
        assertTrue("端到端延迟不应为 0", stats.e2eMs > 0.0)
        assertTrue("掉帧数不应为 0", stats.droppedFrames > 0L)
    }

    @Test
    fun `统计里的分辨率与帧率由协商结果补齐`() {
        val negotiated = StreamCfg(width = 2560, height = 1440, fps = 120, codec = VideoCodec.H264)
        val raw = GenEvent.Stats(
            GenStats(
                sessionId = 1uL,
                fps = 119u,
                bitrateBps = 1u,
                lossPpm = 0u,
                rttUs = 1u,
                decodeUs = 1u,
                renderUs = 1u,
                e2eUs = 1u,
                droppedFrames = 0u,
            ),
        )

        val stats = NativeMappings.mapStats(raw, negotiated)

        assertEquals(2560, stats.width)
        assertEquals(1440, stats.height)
        assertEquals(120, stats.fpsTarget)
        assertEquals(VideoCodec.H264, stats.codec)
    }

    // --------------------------------------------------------------- 事件

    @Test
    fun `状态变化事件被映射`() {
        val event = NativeMappings.mapSessionEvent(
            GenEvent.StateChanged(dev.beam.core.net.fakes.SessionState.Streaming),
        )
        assertTrue(event is SessionEvent.StateChanged)
        assertEquals(SessionState.STREAMING, (event as SessionEvent.StateChanged).state)
    }

    @Test
    fun `丢帧事件被映射且不是错误`() {
        // §3.2 E2004：丢帧只作统计 / 提示，**不能**被映射成 Error。
        val event = NativeMappings.mapSessionEvent(GenEvent.FramesDropped(count = 3u, code = 2004u))
        assertTrue(event is SessionEvent.FramesDropped)
        val dropped = event as SessionEvent.FramesDropped
        assertEquals(3, dropped.count)
        assertEquals("E2004", dropped.code)
    }

    @Test
    fun `错误事件被映射`() {
        val event = NativeMappings.mapSessionEvent(GenEvent.Error("E9002", "主机拒绝了连接", true))
        assertTrue(event is SessionEvent.Error)
        val error = event as SessionEvent.Error
        assertEquals("E9002", error.code)
        assertEquals("主机拒绝了连接", error.message)
        assertTrue(error.retryable)
    }

    @Test
    fun `Closed 变体映射为连接已断开`() {
        val event = NativeMappings.mapSessionEvent(GenEvent.Closed)
        assertTrue(event is SessionEvent.Disconnected)
        assertFalse((event as SessionEvent.Disconnected).retryable)
    }

    @Test
    fun `协商完成事件带回最终配置`() {
        val event = NativeMappings.mapSessionEvent(
            GenEvent.Negotiated(
                GenCfg(
                    width = 1920,
                    height = 1080,
                    fps = 60,
                    codec = "HEVC",
                    bitrateKbps = 15_000,
                    fecRatio = 0.2f,
                    audioEnabled = true,
                ),
            ),
        )
        assertTrue(event is SessionEvent.Negotiated)
        val cfg = (event as SessionEvent.Negotiated).cfg
        assertEquals(1920, cfg.width)
        assertEquals(1080, cfg.height)
        assertEquals(60, cfg.fps)
        assertEquals(VideoCodec.HEVC, cfg.codec)
        assertEquals(15_000, cfg.bitrateKbps)
        assertTrue(cfg.audioEnabled)
    }

    @Test
    fun `无法识别的变体返回 null`() {
        assertNull(NativeMappings.mapSessionEvent(object {}))
    }

    // --------------------------------------------------------------- 发现

    @Test
    fun `发现结果把地址与端口拼成 host 端口`() {
        val host = NativeMappings.mapDiscoveredHost(
            GenHost(
                id = "host-1",
                name = "客厅主机",
                version = "0.1.0",
                fingerprint = "ab12cd34",
                port = 4827u,
                address = "192.168.1.10",
                fullname = "beam-1._beam._udp.local",
            ),
        )

        assertEquals("192.168.1.10:4827", host.peer.address)
        assertEquals("host-1", host.peer.id)
        assertEquals("客厅主机", host.peer.name)
        assertEquals("ab12cd34", host.peer.fingerprint)
        assertEquals(DiscoverySource.MDNS, host.discoveredBy)
    }

    @Test
    fun `发现结果在地址已含端口时不重复拼接`() {
        val host = NativeMappings.mapDiscoveredHost(
            GenHost(
                id = "host-2",
                name = "主机",
                version = "0.1.0",
                fingerprint = "",
                port = 4827u,
                address = "192.168.1.11:9000",
                fullname = "",
            ),
        )
        assertEquals("192.168.1.11:9000", host.peer.address)
    }

    // --------------------------------------------------------------- 配对

    @Test
    fun `扫码配对成功时 uniffi 直接返回 peer 记录`() {
        val result = NativeMappings.mapPairResult(
            GenPeer(
                id = "host-1",
                name = "主机",
                address = "192.168.1.10:4827",
                fingerprint = "ab12",
                platform = "windows",
                trusted = true,
            ),
        )
        assertTrue(result is PairResult.Success)
        val peer = (result as PairResult.Success).peer
        assertEquals("192.168.1.10:4827", peer.address)
        assertTrue(peer.trusted)
    }

    @Test
    fun `PIN 配对成功时 uniffi 返回 Unit`() {
        val requested = Peer("id", "name", "192.168.1.10:4827", "fp")
        val result = NativeMappings.mapPairResult(Unit, requested)
        assertTrue(result is PairResult.Success)
        assertEquals(requested, (result as PairResult.Success).peer)
    }

    @Test
    fun `PIN 失败时按 uniffi 异常类型映射错误码`() {
        // uniffi 生成的 `BeamException.PinFailed` 的 message 是空串，
        // 因此只能按异常类型名映射错误码。
        val thrown = BeamNativeException("调用 uniffi 方法 pairPin 失败", PinFailed())
        val failure = NativeMappings.pairFailure(thrown)
        assertEquals("E14002", failure.code)
        assertNotNull(failure.message)
    }

    /** 模拟 uniffi 生成的 `BeamException.PinFailed`（异常类名决定错误码）。 */
    private class PinFailed : RuntimeException("")

    // --------------------------------------------------------------- 工具

    @Test
    fun `编解码器名大小写不敏感`() {
        assertEquals(VideoCodec.H264, NativeMappings.parseVideoCodec("H264"))
        assertEquals(VideoCodec.H264, NativeMappings.parseVideoCodec("avc"))
        assertEquals(VideoCodec.HEVC, NativeMappings.parseVideoCodec("hevc"))
        assertEquals(VideoCodec.AV1, NativeMappings.parseVideoCodec("AV1"))
        assertEquals(VideoCodec.HEVC, NativeMappings.parseVideoCodec("未知"))
    }

    @Test
    fun `会话状态名大小写不敏感`() {
        assertEquals(SessionState.STREAMING, NativeMappings.parseSessionState("Streaming"))
        assertEquals(SessionState.IDLE, NativeMappings.parseSessionState("idle"))
        assertEquals(SessionState.IDLE, NativeMappings.parseSessionState("完全不是状态"))
    }
}
