package dev.beam.core.net

/**
 * 帧与统计的数据模型。
 *
 * 帧的**热路径**数据以 `java.nio.ByteBuffer`（DirectByteBuffer）形式在 JNI 边界上传递，
 * 避免 uniffi 的 `Vec<u8>` 拷贝；控制面事件走 uniffi 对象。
 */

/**
 * 一帧已复用的直接内存缓冲。
 *
 * [lease] 负责在解码器消费完毕后把缓冲归还给 [dev.beam.core.net.buffer.ByteBufferPool]，
 * 因此使用方必须 `close()`（或用 `use {}`）。
 */
class PooledFrame internal constructor(
    val buffer: java.nio.ByteBuffer,
    val ptsUs: Long,
    val captureTsUs: Long,
    val key: Boolean,
    private val lease: dev.beam.core.net.buffer.BufferLease,
) : AutoCloseable {
    override fun close() = lease.close()
}

/**
 * 会话统计，对应 §3.11 事件 `session://stats`（500 ms 一次）。
 *
 * 仅承载客户端可观测的字段；主机侧字段由 Rust 侧汇总后一并下发。
 */
data class SessionStats(
    val fps: Double = 0.0,
    val bitrateKbps: Double = 0.0,
    val rttMs: Double = 0.0,
    val lossPercent: Double = 0.0,
    val fecRecoveredPercent: Double = 0.0,
    val decodeMs: Double = 0.0,
    val renderMs: Double = 0.0,
    val e2eMs: Double = 0.0,
    val droppedFrames: Long = 0,
    val width: Int = 0,
    val height: Int = 0,
    val fpsTarget: Int = 0,
    val codec: VideoCodec = VideoCodec.HEVC,
)

/**
 * 音频包（Opus 5 ms 帧），由会话在 v1 音频阶段下送。
 *
 * 音频与视频一样走 datagram，**不经过 uniffi 对象**，直接以字节数组承载。
 */
data class AudioPacket(
    val data: ByteArray,
    val ptsUs: Long,
) {
    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (other !is AudioPacket) return false
        return ptsUs == other.ptsUs && data.contentEquals(other.data)
    }

    override fun hashCode(): Int = 31 * data.contentHashCode() + ptsUs.hashCode()
}

/**
 * 重连信息（§4.1「3 s 无帧 → Reconnecting」）。
 *
 * 对应 Rust 侧 `beam_session::ReconnectingInfo`，
 * 经 uniffi 的 `Session::reconnecting()` 查询得到。
 *
 * 不在重连态时该查询返回 `null`（而不是一个「空」的重连信息），
 * 因此上层可以直接用 `null` 判断「有没有在重连」。
 */
data class ReconnectingInfo(
    /** 已经重连了多久（毫秒）。 */
    val elapsedMs: Long = 0L,
    /**
     * 是否保留最后一帧画面。
     *
     * 恒为 `true`：会话层在重连期间**从不**要求清屏（§4.1），
     * 最后一帧一直留在屏幕上，直到新帧到达把它覆盖掉。
     */
    val keepLastFrame: Boolean = true,
)

/**
 * 会话汇总（§3.9「统计汇总」）。
 *
 * 对应 Rust 侧 `beam_session::SessionSummary`，经 uniffi 的
 * `Session::summary()` 查询得到。与 [SessionStats] 的区别：
 * [SessionStats] 是**会话内**每 500 ms 一拍的指标流，
 * 本模型是**会话结束后依然可查**的累计值。
 *
 * 单位换算与 [SessionStats] 一致：Rust 用整数最小单位
 * （µs / ppm / bit·s⁻¹），Kotlin 侧用 ms / 百分比 / kbit·s⁻¹。
 */
data class SessionSummary(
    /** 会话号。 */
    val sessionId: Long = 0L,
    /** 会话时长（毫秒）。 */
    val durationMs: Long = 0L,
    /** 已接收的画面帧数。 */
    val frames: Long = 0L,
    /** 累计丢帧数（§3.2 E2004）。 */
    val droppedFrames: Long = 0L,
    /** 重连次数。 */
    val reconnects: Int = 0,
    /** 最近一次上报的往返时延（毫秒）。 */
    val rttMs: Double = 0.0,
    /** 最近一次上报的丢包率（百分比）。 */
    val lossPercent: Double = 0.0,
    /** 最近一次上报的实际帧率。 */
    val fps: Double = 0.0,
    /** 最近一次上报的实际码率（kbit/s）。 */
    val bitrateKbps: Double = 0.0,
    /** 会话当前（或结束时的）状态。 */
    val state: SessionState = SessionState.IDLE,
)
