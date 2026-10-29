package dev.beam.core.audio

/**
 * 单生产者 / 单消费者 PCM 环形缓冲。
 *
 * Opus 解码线程往里写，输出线程从里面取。两端都用短临界区，
 * 不在热路径上分配内存（预分配一块数组）。
 *
 * 用**显式计数器**而不是用 `head`/`tail` 推导长度：`head == tail` 既可能是
 * 「空」也可能是「满」，推导会出错。
 *
 * 纯 Kotlin 实现，方便做单元测试。
 */
class PcmRingBuffer(
    /** 缓冲容量（字节）。 */
    val capacityBytes: Int,
) {

    init {
        require(capacityBytes > 0) { "容量必须为正数" }
    }

    private val buffer = ByteArray(capacityBytes)
    private val lock = Any()
    private var head = 0
    private var tail = 0
    private var count = 0

    /** 可读字节数。 */
    fun readableBytes(): Int = synchronized(lock) { count }

    /** 可写字节数。 */
    fun writableBytes(): Int = capacityBytes - readableBytes()

    /**
     * 写入数据。
     *
     * @return 实际写入的字节数（空间不足时写入前半段）。
     */
    fun write(data: ByteArray, offset: Int = 0, length: Int = data.size - offset): Int {
        if (length <= 0) return 0
        synchronized(lock) {
            val n = minOf(length, capacityBytes - count)
            var remaining = n
            var src = offset
            while (remaining > 0) {
                val chunk = minOf(remaining, capacityBytes - tail)
                System.arraycopy(data, src, buffer, tail, chunk)
                tail = (tail + chunk) % capacityBytes
                src += chunk
                remaining -= chunk
            }
            count += n
            return n
        }
    }

    /**
     * 读取数据。
     *
     * @return 实际读取的字节数。
     */
    fun read(target: ByteArray, offset: Int = 0, length: Int = target.size - offset): Int {
        if (length <= 0) return 0
        synchronized(lock) {
            val n = minOf(length, count)
            var remaining = n
            var dst = offset
            while (remaining > 0) {
                val chunk = minOf(remaining, capacityBytes - head)
                System.arraycopy(buffer, head, target, dst, chunk)
                head = (head + chunk) % capacityBytes
                dst += chunk
                remaining -= chunk
            }
            count -= n
            return n
        }
    }

    /** 清空缓冲。 */
    fun clear() = synchronized(lock) {
        head = 0
        tail = 0
        count = 0
    }
}
