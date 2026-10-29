package dev.beam.core.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat

/**
 * Opus 解码 + 低延迟输出（§3.12 `:core-audio`）。
 *
 * ## 管线
 * ```
 * Opus 包 ──▶ MediaCodec(audio/opus) ──▶ PCM16 ──▶ AudioTrack(MODE_STREAM, 低延迟)
 * ```
 *
 * ## 关于 AAudio
 *
 * **AAudio 在公开 Android SDK 里没有 Java/Kotlin 绑定**（`AAudio.h` 只随 NDK
 * 分发，`android.jar` 中不存在 `AAudio` / `AAudioStream` 类）。因此 Java 侧走
 * `AudioTrack` 的低延迟路径：
 *
 * - `AudioTrack.PERFORMANCE_MODE_LOW_LATENCY`（API 26+）；
 * - `AudioAttributes.USAGE_GAME` + `CONTENT_TYPE_MOVIE`；
 * - `MODE_STREAM` + 小缓冲。
 *
 * 该组合在系统内部会路由到与 AAudio 相同的原生 fast mixer 快速通道
 * （`AudioTrack` 的低延迟模式即建立在其上），延迟量级与 AAudio 一致。
 *
 * 若后续确实需要直接持有 AAudio 流（例如自定义 `AAudioStream` 回调、
 * 独占 `AAudioStream` 的 `setFormat` 协商），需要在 `:core-audio` 里补一个
 * NDK/JNI 薄封装（CMake + `AAudio.h`），Kotlin 侧接口无需改动。
 *
 * ## 线程模型
 * - `enqueue`：音频包投递线程（同步解码，Opus 帧 5 ms）；
 * - `write`：同一个投递线程（`AudioTrack` 阻塞写，缓冲满时自然背压）；
 * - `stop` / `release`：任意线程。
 */
class LowLatencyAudioPlayer(
    private val onError: (code: String, message: String) -> Unit = { _, _ -> },
) : AudioPlayer {

    private var codec: MediaCodec? = null
    private var track: AudioTrack? = null
    private var format: PcmAudioFormat = PcmAudioFormat()

    override fun start(format: PcmAudioFormat) {
        stop()
        this.format = format

        codec = createOpusDecoder(format)

        val channelMask = if (format.channels == 1) {
            AudioFormat.CHANNEL_OUT_MONO
        } else {
            AudioFormat.CHANNEL_OUT_STEREO
        }
        val minBytes = AudioTrack.getMinBufferSize(
            format.sampleRate,
            channelMask,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        val bufferBytes = if (minBytes > 0) minBytes * 2 else DEFAULT_BUFFER_BYTES

        val created = try {
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_GAME)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                        .build(),
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(format.sampleRate)
                        .setChannelMask(channelMask)
                        .build(),
                )
                .setBufferSizeInBytes(bufferBytes)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .build()
        } catch (e: Exception) {
            onError("E70003", "创建音频输出流失败：${e.message}")
            null
        }
        if (created == null) {
            releaseDecoder()
            return
        }
        track = created
        runCatching { created.play() }
            .onFailure { onError("E70006", "启动音频输出失败：${it.message}") }
    }

    override fun enqueue(packet: ByteArray, ptsUs: Long) {
        val decoder = codec ?: return
        try {
            val index = decoder.dequeueInputBuffer(DEQUEUE_TIMEOUT_US)
            if (index < 0) return
            val input = decoder.getInputBuffer(index) ?: return
            input.clear()
            input.put(packet)
            decoder.queueInputBuffer(index, 0, packet.size, ptsUs, 0)
            drainDecoded(decoder)
        } catch (e: Exception) {
            onError("E70002", "Opus 解码失败：${e.message}")
        }
    }

    override fun stop() {
        track?.let {
            runCatching { it.pause() }
            runCatching { it.flush() }
            runCatching { it.release() }
        }
        track = null
        releaseDecoder()
    }

    override fun release() {
        stop()
    }

    // ------------------------------------------------------------- 内部实现

    private fun releaseDecoder() {
        codec?.let {
            runCatching { it.stop() }
            runCatching { it.release() }
        }
        codec = null
    }

    private fun createOpusDecoder(format: PcmAudioFormat): MediaCodec? = try {
        val mime = "audio/opus"
        val mediaFormat = MediaFormat.createAudioFormat(mime, format.sampleRate, format.channels).apply {
            setInteger(MediaFormat.KEY_PCM_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
        }
        MediaCodec.createDecoderByType(mime).apply {
            configure(mediaFormat, null, null, 0)
            start()
        }
    } catch (e: Exception) {
        onError("E70001", "创建 Opus 解码器失败：${e.message}")
        null
    }

    private fun drainDecoded(decoder: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        while (true) {
            val outIndex = decoder.dequeueOutputBuffer(info, 0)
            if (outIndex < 0) break
            try {
                if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                    decoder.releaseOutputBuffer(outIndex, false)
                    break
                }
                if (info.size > 0) {
                    val out = decoder.getOutputBuffer(outIndex)
                    val sink = track
                    if (out != null && sink != null) {
                        val bytes = ByteArray(info.size)
                        out.get(bytes)
                        var offset = 0
                        while (offset < bytes.size) {
                            val written = sink.write(bytes, offset, bytes.size - offset)
                            if (written <= 0) break
                            offset += written
                        }
                    }
                }
            } finally {
                decoder.releaseOutputBuffer(outIndex, false)
            }
        }
    }

    companion object {
        /** 默认输出缓冲（字节）：约 250 ms @48kHz/16bit/2ch。 */
        const val DEFAULT_BUFFER_BYTES: Int = 48_000 * 4 * 8

        /** MediaCodec 输入缓冲等待超时（微秒）。 */
        const val DEQUEUE_TIMEOUT_US: Long = 5_000
    }
}
