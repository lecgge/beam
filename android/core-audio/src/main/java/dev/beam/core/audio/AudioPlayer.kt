package dev.beam.core.audio

/**
 * PCM 输出格式。
 *
 * 命名带 `Pcm` 前缀是为了避免与 `android.media.AudioFormat` 混淆。
 */
data class PcmAudioFormat(
    /** 采样率（Hz）；Beam 固定 48 kHz（§3.7）。 */
    val sampleRate: Int = 48_000,
    /** 声道数。 */
    val channels: Int = 2,
    /** 每采样点字节数（16 bit = 2）。 */
    val bytesPerSample: Int = 2,
) {
    init {
        require(sampleRate > 0) { "采样率必须为正数" }
        require(channels > 0) { "声道数必须为正数" }
    }

    /** 每帧字节数（所有声道合计）。 */
    val bytesPerFrame: Int get() = bytesPerSample * channels
}

/**
 * 音频播放器（§3.12 `:core-audio` 的对外接口）。
 *
 * 职责：Opus 解码 + AAudio 输出。
 */
interface AudioPlayer {

    /** 启动播放（建立解码器与输出流）。 */
    fun start(format: PcmAudioFormat)

    /**
     * 投送一个 Opus 包。
     *
     * @param packet Opus 负载（§3.7：5 ms 一帧）。
     * @param ptsUs  播放时间戳（单调时钟微秒）。
     */
    fun enqueue(packet: ByteArray, ptsUs: Long)

    /** 停止播放并释放解码器；输出流保持可复用。 */
    fun stop()

    /** 释放全部资源。 */
    fun release()
}
