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
