package dev.beam.core.decoder

import android.media.MediaCodec
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.view.Surface
import java.nio.ByteBuffer
import java.util.ArrayDeque

/**
 * 基于 MediaCodec 异步模式的解码器实现。
 *
 * 特性（§3.12）：
 * - 异步模式（`MediaCodec.Callback` + 专用 HandlerThread）；
 * - `MediaFormat.KEY_LOW_LATENCY = 1`；
 * - 优先选用厂商 low_latency 硬解解码器；
 * - output buffer 直接 `releaseOutputBuffer(index, true)` 渲染到 `Surface`
 *   （由系统处理 output fence，避免额外拷贝）。
 *
 * ## 线程模型
 * - `start` / `queue` / `stop`：调用方线程；
 * - `onInputBufferAvailable` / `onOutputBufferAvailable`：内部 Handler 线程。
 *
 * ## 禁止事项
 * 不做任何网络操作（不依赖 `:core-net`）。
 */
class MediaCodecVideoDecoder(
    /** 强制使用指定解码器名；为 null 时自动探测（§3.12「自动探测」）。 */
    private val preferredDecoderName: String? = null,
    /** 优先低延迟解码器。 */
    private val preferLowLatency: Boolean = true,
    private val onError: (code: String, message: String) -> Unit = { _, _ -> },
) : VideoDecoder {

    private class PendingFrame(val data: ByteBuffer, val ptsUs: Long, val key: Boolean)

    private val lock = Any()
    private val pending = ArrayDeque<PendingFrame>()

    /**
     * 输出栅栏（§3.12「output fence」）。
     *
     * 公开以便上层观测输出侧统计（在途缓冲索引 / 已渲染帧数 / 跳过的 config 缓冲数）。
     */
    val outputFence: OutputFence = OutputFence()

    private var codec: MediaCodec? = null
    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    private var freeInputIndex: Int = -1
    private var surface: Surface? = null

    override fun start(surface: Surface, fmt: VideoFormat) {
        stop()
        this.surface = surface

        val codecName = resolveDecoderName(fmt.codec)
        val mediaCodec = if (codecName != null) {
            runCatching { MediaCodec.createByCodecName(codecName) }.getOrElse {
                onError("E10001", "创建解码器 $codecName 失败：${it.message}")
                return
            }
        } else {
            runCatching { MediaCodec.createDecoderByType(fmt.codec.mime) }.getOrElse {
                onError("E10002", "创建 ${fmt.codec.mime} 解码器失败：${it.message}")
                return
            }
        }

        val format = MediaFormat.createVideoFormat(fmt.codec.mime, fmt.width, fmt.height).apply {
            setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_FRAME_RATE, fmt.fps)
            setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, MAX_INPUT_SIZE)
            if (fmt.profile > 0) setInteger(MediaFormat.KEY_PROFILE, fmt.profile)
            if (fmt.level > 0) setInteger(MediaFormat.KEY_LEVEL, fmt.level)
        }

        try {
            mediaCodec.configure(format, surface, null, 0)
            // configure 之后、start 之前下发低延迟参数（部分厂商只认这里）。
            runCatching { mediaCodec.setParameters(DeviceDecoders.lowLatencyParams()) }
        } catch (e: Exception) {
            runCatching { mediaCodec.release() }
            onError("E10003", "配置解码器失败：${e.message}")
            return
        }

        val cbThread = HandlerThread("beam-decoder").apply { start() }
        val cbHandler = Handler(cbThread.looper)
        thread = cbThread
        handler = cbHandler

        mediaCodec.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
                synchronized(lock) {
                    freeInputIndex = index
                }
                pump(codec)
            }

            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                // 输出栅栏：登记在途缓冲与显示时间戳，并判断该缓冲是否该上屏。
                // codec-config 缓冲（SPS/PPS）**不能**渲染到 Surface。
                val render = outputFence.begin(
                    bufferIndex = index,
                    presentationTimeUs = info.presentationTimeUs,
                    isCodecConfig = isCodecConfig(info.flags),
                )
                // 渲染到 Surface：框架在内部完成 output fence 同步（公开 SDK 不暴露 fence）。
                runCatching { codec.releaseOutputBuffer(index, render) }
                outputFence.complete(index)
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) = Unit

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                onError("E10004", "解码器异常：${e.diagnosticInfo ?: e.message}")
            }
        }, cbHandler)

        try {
            mediaCodec.start()
        } catch (e: Exception) {
            runCatching { mediaCodec.release() }
            cbThread.quitSafely()
            thread = null
            handler = null
            onError("E10005", "启动解码器失败：${e.message}")
            return
        }
        codec = mediaCodec
    }

    override fun queue(frame: ByteBuffer, ptsUs: Long, key: Boolean) {
        val mediaCodec = codec ?: return
        synchronized(lock) {
            pending.addLast(PendingFrame(frame, ptsUs, key))
        }
        pump(mediaCodec)
    }

    override fun stop() {
        val mediaCodec = codec
        codec = null
        if (mediaCodec != null) {
            runCatching {
                mediaCodec.stop()
            }
            runCatching { mediaCodec.release() }
        }
        synchronized(lock) {
            pending.clear()
            freeInputIndex = -1
        }
        outputFence.reset()
        thread?.quitSafely()
        thread = null
        handler = null
        surface = null
    }

    // ------------------------------------------------------------- 内部实现

    /**
     * 把待解码帧填入可用的输入缓冲区。
     *
     * 整段在锁内完成，保证 `queueInputBuffer` 不会被并发调用。
     */
    private fun pump(mediaCodec: MediaCodec) {
        while (true) {
            val frame: PendingFrame
            val index: Int
            synchronized(lock) {
                if (freeInputIndex < 0 || pending.isEmpty()) return
                index = freeInputIndex
                freeInputIndex = -1
                frame = pending.removeFirst()
            }
            try {
                val input = mediaCodec.getInputBuffer(index) ?: continue
                input.clear()
                val size = minOf(frame.data.remaining(), input.capacity())
                val slice = frame.data.duplicate()
                slice.limit(slice.position() + size)
                input.put(slice)
                val flags = if (frame.key) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
                mediaCodec.queueInputBuffer(index, 0, size, frame.ptsUs, flags)
            } catch (e: Exception) {
                onError("E10006", "提交解码帧失败：${e.message}")
                return
            }
        }
    }

    private fun resolveDecoderName(codec: DecoderCodec): String? {
        preferredDecoderName?.let { return it }
        val candidates = runCatching { DeviceDecoders.candidates(codec) }.getOrDefault(emptyList())
        if (candidates.isEmpty()) {
            // 退回系统默认。
            return null
        }
        return DecoderSelector.select(candidates, codec, preferLowLatency)?.name
    }

    /** 是否是 codec-config 缓冲（SPS/PPS 等，**不能**渲染到 Surface）。 */
    private fun isCodecConfig(flags: Int): Boolean =
        flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0

    companion object {
        /** 单帧最大输入字节数：覆盖 4K60 的 IDR（§5 单帧上限）。 */
        const val MAX_INPUT_SIZE: Int = 4 shl 20

        /** 便于测试/诊断：列出设备上支持该编码的所有解码器名。 */
        fun decoderNames(codec: DecoderCodec): List<String> =
            MediaCodecList(MediaCodecList.REGULAR_CODECS)
                .codecInfos
                .filter { info -> info.supportedTypes.any { it.equals(codec.mime, ignoreCase = true) } }
                .map { it.name }
    }
}
