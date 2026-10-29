package dev.beam.core.decoder

import android.view.Surface
import java.nio.ByteBuffer

/**
 * 视频解码器（§3.12 规定的对外签名）。
 *
 * ```kotlin
 * interface VideoDecoder {
 *     fun start(surface: Surface, fmt: VideoFormat)
 *     fun queue(frame: ByteBuffer, ptsUs: Long, key: Boolean)
 *     fun stop()
 * }
 * ```
 *
 * ## 职责
 * MediaCodec 异步模式、`KEY_LOW_LATENCY`、厂商 low_latency 解码器优选、output fence 处理。
 *
 * ## 禁止事项（§3.12）
 * **不得依赖网络层**：本模块不依赖 `:core-net`，也不做任何传输相关的事情。
 * 输入是调用方交过来的 `ByteBuffer`，本模块只管解码到 `Surface`。
 *
 * ## 线程模型
 * `start` / `queue` / `stop` 由调用方（`:feature-stream`）在**帧回调线程**
 * 调用；MediaCodec 的异步回调发生在内部 Handler 线程。
 * `queue` 不得阻塞等待输入缓冲区。
 */
interface VideoDecoder {

    /**
     * 启动解码器。
     *
     * @param surface 输出表面（由 `:core-renderer` 提供）。
     * @param fmt     流格式（分辨率 / 编码 / profile / level）。
     */
    fun start(surface: Surface, fmt: VideoFormat)

    /**
     * 提交一帧压缩数据。
     *
     * @param frame 压缩视频数据；实现方**不得**长期持有该缓冲（需要时自行拷贝）。
     * @param ptsUs  显示时间戳（单调时钟微秒，§6 约定）。
     * @param key    是否为关键帧（IDR）。
     */
    fun queue(frame: ByteBuffer, ptsUs: Long, key: Boolean)

    /** 停止解码并释放资源。可重复调用。 */
    fun stop()
}

/**
 * 流格式。
 *
 * 与 `beam-proto` 的 `StreamConfig` / `VideoPktHeader` 语义对应，但**故意不依赖**
 * `:core-net`（§3.12 禁止依赖网络）。
 */
data class VideoFormat(
    val width: Int,
    val height: Int,
    val codec: DecoderCodec,
    /** H.264 / HEVC profile，-1 表示不指定。 */
    val profile: Int = -1,
    /** H.264 / HEVC level，-1 表示不指定。 */
    val level: Int = -1,
    /** 目标帧率（用于 `KEY_FRAME_RATE`）。 */
    val fps: Int = 60,
) {
    init {
        require(width > 0 && height > 0) { "分辨率必须为正数" }
    }
}

/** 容器编码格式。 */
enum class DecoderCodec(val mime: String) {
    H264("video/avc"),
    HEVC("video/hevc"),
    AV1("video/av1"),
    ;
}
