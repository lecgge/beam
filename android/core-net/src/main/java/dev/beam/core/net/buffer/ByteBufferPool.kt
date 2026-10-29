package dev.beam.core.net.buffer

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * DirectByteBuffer 池。
 *
 * §6 内存所有权约定：「JNI 热路径 DirectByteBuffer 由 Rust 侧池化，回调返回后归还」。
 * Kotlin 侧的 [ByteBufferPool] 承担同样的职责：
 *
 *  1. [rent] 取得一块**直接内存**缓冲（供 JNI 侧写入，避免一次数组拷贝）；
 *  2. 消费方用完调用 [BufferLease.close]，缓冲归还池中复用。
 *
 * 本池是**线程安全**的，但 [rent] / [release] 应尽量在同一线程配对使用，
 * 避免跨线程归还带来的可见性开销。
 */
class ByteBufferPool(
    /** 池中最多保留的空闲缓冲个数。 */
    private val maxIdleBuffers: Int = 8,
    /** 单块缓冲的默认字节数。 */
    private val defaultCapacityBytes: Int = DEFAULT_CAPACITY,
) {

    private val idle = ArrayDeque<ByteBuffer>()
    private var createdCount = 0
    private var reusedCount = 0

    /**
     * 取得一块容量至少为 [capacityBytes] 的直接内存缓冲。
     *
     * @param capacityBytes 需要的字节数；小于等于 0 时使用默认容量。
     * @param lease 是否返回带归还逻辑的 [BufferLease]（`true`）还是裸缓冲（`false`）。
     */
    fun rent(capacityBytes: Int = defaultCapacityBytes, lease: Boolean = true): ByteBuffer {
        val want = if (capacityBytes <= 0) defaultCapacityBytes else capacityBytes
        val buffer = synchronized(idle) {
            val idx = idle.indexOfFirst { it.capacity() >= want }
            if (idx >= 0) {
                reusedCount++
                idle.removeAt(idx).apply {
                    clear()
                    order(ByteOrder.BIG_ENDIAN)
                }
            } else {
                createdCount++
                ByteBuffer.allocateDirect(want).order(ByteOrder.BIG_ENDIAN)
            }
        }
        return if (lease) buffer else buffer
    }

    /** 取得一个带归还句柄的租约。 */
    fun rentLease(capacityBytes: Int = defaultCapacityBytes): BufferLease =
        BufferLease(this, rent(capacityBytes, lease = false))

    /**
     * 归还一块缓冲。仅当缓冲是 [ByteBuffer.isDirect] 且容量合理时才回池，
     * 否则丢弃让 GC 回收。
     */
    fun release(buffer: ByteBuffer) {
        if (!buffer.isDirect) return
        if (buffer.capacity() > MAX_POOLED_CAPACITY) return
        synchronized(idle) {
            if (idle.size >= maxIdleBuffers) return
            idle.addLast(buffer)
        }
    }

    /** 池内空闲缓冲数。 */
    val idleCount: Int get() = synchronized(idle) { idle.size }

    /** 累计创建的缓冲数（用于诊断）。 */
    val created: Int get() = synchronized(idle) { createdCount }

    /** 累计复用次数（用于诊断）。 */
    val reused: Int get() = synchronized(idle) { reusedCount }

    /** 清空空闲池。 */
    fun clear() {
        synchronized(idle) { idle.clear() }
    }

    companion object {
        /** 单帧缓冲默认容量：覆盖 4K60 的单分片上限，余量充足。 */
        const val DEFAULT_CAPACITY: Int = 1 shl 20

        /** 超过该容量的缓冲不回收，避免长期占用大块直接内存。 */
        const val MAX_POOLED_CAPACITY: Int = 4 shl 20
    }
}

/**
 * 缓冲租约。
 *
 * 典型用法：
 * ```kotlin
 * pool.rentLease().use { lease ->
 *     // 使用 lease.buffer
 * }
 * ```
 */
class BufferLease internal constructor(
    private val pool: ByteBufferPool,
    val buffer: ByteBuffer,
) : AutoCloseable {

    private var released = false

    /** 是否已归还。 */
    val isReleased: Boolean get() = released

    override fun close() {
        if (released) return
        released = true
        pool.release(buffer)
    }
}
