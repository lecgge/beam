package dev.beam.core.decoder

/**
 * 输出栅栏（output fence）。
 *
 * ## 背景
 *
 * MediaCodec 的**公开 SDK 不暴露** Surface 输出的原始 fence：框架在
 * `onOutputBufferAvailable()` 回调之前就已经等过内部 fence，
 * 开发者拿不到 `SyncFence` 对象（§3.12 要求的「output fence」在公开 SDK 上只能这样落地）。
 *
 * 但「哪个输出缓冲在途」「这一帧该不该上屏」这两件事此前完全没人管：
 * `MediaCodecVideoDecoder` 直接 `releaseOutputBuffer(index, true)`，
 * 连 `MediaCodec.BufferInfo` 都没看 —— 于是 **codec-config 缓冲也被当成画面渲染上屏**。
 *
 * ## 本类做什么
 *
 * 把输出侧的状态**显式化**并做到可单测：
 * - [begin]：登记一次输出缓冲（索引 + 显示时间戳 + 是否 codec config）；
 * - [complete]：登记该缓冲已交还 MediaCodec；
 * - [outputBufferIndex] / [renderedCount] / [configCount]：供上层观测与统计。
 *
 * ## 线程模型
 * 无状态共享要求，但由解码器的回调线程写入；读取方（统计 / UI）可能来自别的线程，
 * 因此计数用 `Long` 并按「单写多读」使用。
 */
class OutputFence {

    /**
     * 当前在途（已提交、尚未交还）的输出缓冲索引；没有在途缓冲时为 [NONE]。
     */
    var outputBufferIndex: Int = NONE
        private set

    /** 在途缓冲的显示时间戳（微秒）。 */
    var outputPtsUs: Long = 0L
        private set

    /** 已渲染（上屏）的帧数。 */
    var renderedCount: Long = 0L
        private set

    /** 被跳过的 codec-config 缓冲数。 */
    var configCount: Long = 0L
        private set

    /**
     * 登记一次输出缓冲。
     *
     * @param bufferIndex 输出缓冲索引。
     * @param presentationTimeUs 显示时间戳（微秒）。
     * @param isCodecConfig 是否是 codec-config 缓冲（**不能**上屏）。
     * @return true 表示该缓冲应当渲染到 Surface；false 表示应当跳过。
     */
    fun begin(bufferIndex: Int, presentationTimeUs: Long, isCodecConfig: Boolean): Boolean {
        outputBufferIndex = bufferIndex
        outputPtsUs = presentationTimeUs
        return if (isCodecConfig) {
            configCount++
            false
        } else {
            renderedCount++
            true
        }
    }

    /**
     * 登记缓冲已交还 MediaCodec。
     *
     * @param bufferIndex 交还的缓冲索引；与在途索引不一致时忽略（防止错配）。
     */
    fun complete(bufferIndex: Int) {
        if (outputBufferIndex == bufferIndex) {
            outputBufferIndex = NONE
            outputPtsUs = 0L
        }
    }

    /** 重置（解码器重启时调用）。 */
    fun reset() {
        outputBufferIndex = NONE
        outputPtsUs = 0L
        renderedCount = 0L
        configCount = 0L
    }

    companion object {
        /** 「没有在途缓冲」的哨兵值。 */
        const val NONE: Int = -1
    }
}
