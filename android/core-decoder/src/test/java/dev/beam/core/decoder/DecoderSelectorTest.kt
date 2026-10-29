package dev.beam.core.decoder

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [DecoderSelector] 单元测试：解码器选型策略。
 */
class DecoderSelectorTest {

    private fun candidate(
        name: String,
        vendor: String,
        hardware: Boolean = true,
        lowLatency: Boolean = false,
        codecs: Set<DecoderCodec> = setOf(DecoderCodec.HEVC),
    ) = DecoderCandidate(
        name = name,
        vendor = vendor,
        hardwareAccelerated = hardware,
        lowLatency = lowLatency,
        codecs = codecs,
    )

    @Test
    fun `没有支持该编码的解码器时返回 null`() {
        val result = DecoderSelector.select(
            candidates = listOf(candidate("c2.android.hevc.decoder", "c2", codecs = setOf(DecoderCodec.H264))),
            codec = DecoderCodec.HEVC,
        )
        assertNull(result)
    }

    @Test
    fun `优先选择厂商低延迟解码器`() {
        val result = DecoderSelector.select(
            candidates = listOf(
                candidate("c2.android.hevc.decoder", "c2"),
                candidate("c2.qti.hevc.decoder", "qti", lowLatency = true),
            ),
            codec = DecoderCodec.HEVC,
        )
        assertEquals("c2.qti.hevc.decoder", result?.name)
    }

    @Test
    fun `没有低延迟解码器时选择硬解`() {
        val result = DecoderSelector.select(
            candidates = listOf(
                candidate("OMX.google.hevc.decoder", "OMX", hardware = false),
                candidate("c2.qti.hevc.decoder", "qti"),
            ),
            codec = DecoderCodec.HEVC,
        )
        assertEquals("c2.qti.hevc.decoder", result?.name)
    }

    @Test
    fun `软解只在没有硬解时兜底`() {
        val result = DecoderSelector.select(
            candidates = listOf(candidate("OMX.google.hevc.decoder", "OMX", hardware = false)),
            codec = DecoderCodec.HEVC,
        )
        assertEquals("OMX.google.hevc.decoder", result?.name)
    }

    @Test
    fun `关闭低延迟偏好时不优先低延迟解码器`() {
        val result = DecoderSelector.select(
            candidates = listOf(
                candidate("c2.android.hevc.decoder", "c2"),
                candidate("c2.qti.hevc.decoder", "qti", lowLatency = true),
            ),
            codec = DecoderCodec.HEVC,
            preferLowLatency = false,
        )
        // 白名单厂商依然优先
        assertEquals("c2.qti.hevc.decoder", result?.name)
    }

    @Test
    fun `低延迟厂商白名单识别`() {
        assertTrue(DecoderSelector.isKnownLowLatencyVendor("c2.qti.hevc.decoder"))
        assertTrue(DecoderSelector.isKnownLowLatencyVendor("OMX.mtk.hevc.decoder"))
        assertTrue(!DecoderSelector.isKnownLowLatencyVendor("OMX.google.hevc.decoder"))
    }

    @Test
    fun `编码 mime 与协议取值一致`() {
        assertEquals("video/avc", DecoderCodec.H264.mime)
        assertEquals("video/hevc", DecoderCodec.HEVC.mime)
        assertEquals("video/av1", DecoderCodec.AV1.mime)
    }
}
