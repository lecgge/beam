package dev.beam.core.decoder

import android.media.MediaCodec
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [OutputFence] 与 [VideoDecoderFactory] 的单测。
 *
 * `MediaCodec.BUFFER_FLAG_CODEC_CONFIG` 是编译期内联的常量，
 * 因此「哪些缓冲可以上屏」的判定逻辑可以在 JVM 上直接验证。
 */
class OutputFenceTest {

    @Test
    fun `普通缓冲应当渲染并计数`() {
        val fence = OutputFence()
        val render = fence.begin(bufferIndex = 3, presentationTimeUs = 12_345L, isCodecConfig = false)
        assertTrue(render)
        assertEquals(3, fence.outputBufferIndex)
        assertEquals(12_345L, fence.outputPtsUs)
        assertEquals(1L, fence.renderedCount)
        assertEquals(0L, fence.configCount)
    }

    @Test
    fun `codec config 缓冲不渲染但被计数`() {
        val fence = OutputFence()
        val render = fence.begin(bufferIndex = 7, presentationTimeUs = 0L, isCodecConfig = true)
        assertFalse(render)
        assertEquals(0L, fence.renderedCount)
        assertEquals(1L, fence.configCount)
    }

    @Test
    fun `交还缓冲后清空在途索引`() {
        val fence = OutputFence()
        fence.begin(bufferIndex = 5, presentationTimeUs = 100L, isCodecConfig = false)
        fence.complete(5)
        assertEquals(OutputFence.NONE, fence.outputBufferIndex)
        assertEquals(0L, fence.outputPtsUs)
    }

    @Test
    fun `索引不匹配的交还被忽略`() {
        val fence = OutputFence()
        fence.begin(bufferIndex = 5, presentationTimeUs = 100L, isCodecConfig = false)
        fence.complete(6)
        assertEquals(5, fence.outputBufferIndex)
        assertEquals(100L, fence.outputPtsUs)
    }

    @Test
    fun `重置清空全部计数`() {
        val fence = OutputFence()
        fence.begin(bufferIndex = 1, presentationTimeUs = 1L, isCodecConfig = false)
        fence.begin(bufferIndex = 2, presentationTimeUs = 2L, isCodecConfig = true)
        fence.reset()
        assertEquals(OutputFence.NONE, fence.outputBufferIndex)
        assertEquals(0L, fence.renderedCount)
        assertEquals(0L, fence.configCount)
    }

    @Test
    fun `codec config 标志的判定`() {
        val config = MediaCodec.BUFFER_FLAG_CODEC_CONFIG
        val keyFrame = MediaCodec.BUFFER_FLAG_KEY_FRAME
        // 与 VideoDecoder 一致的判定：只有带 config 标志的缓冲才跳过。
        assertTrue(config and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0)
        assertFalse(keyFrame and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0)
    }

    @Test
    fun `编码名解析成容器格式`() {
        assertEquals(DecoderCodec.H264, VideoDecoderFactory.codecOf("H264"))
        assertEquals(DecoderCodec.H264, VideoDecoderFactory.codecOf("h264"))
        assertEquals(DecoderCodec.HEVC, VideoDecoderFactory.codecOf("HEVC"))
        assertEquals(DecoderCodec.AV1, VideoDecoderFactory.codecOf("AV1"))
        assertEquals(DecoderCodec.HEVC, VideoDecoderFactory.codecOf("不认识"))
    }

    @Test
    fun `工厂按编码名构造流格式`() {
        val format = VideoDecoderFactory.format(1920, 1080, "H264", 60)
        assertEquals(1920, format.width)
        assertEquals(1080, format.height)
        assertEquals(DecoderCodec.H264, format.codec)
        assertEquals(60, format.fps)
    }
}
