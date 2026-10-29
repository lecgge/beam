package dev.beam.core.audio

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * [PcmRingBuffer] 单元测试。
 */
class PcmRingBufferTest {

    @Test
    fun `写入后可读回相同数据`() {
        val ring = PcmRingBuffer(64)
        val source = ByteArray(10) { it.toByte() }
        assertEquals(10, ring.write(source))
        assertEquals(10, ring.readableBytes())

        val target = ByteArray(10)
        assertEquals(10, ring.read(target))
        assertArrayEquals(source, target)
        assertEquals(0, ring.readableBytes())
    }

    @Test
    fun `空间不足时只写入前半段`() {
        val ring = PcmRingBuffer(8)
        val source = ByteArray(20) { it.toByte() }
        assertEquals(8, ring.write(source))
        assertEquals(0, ring.writableBytes())
    }

    @Test
    fun `环形回绕后仍能正确读写`() {
        val ring = PcmRingBuffer(8)
        ring.write(byteArrayOf(1, 2, 3, 4, 5, 6))
        val first = ByteArray(4)
        ring.read(first)
        assertArrayEquals(byteArrayOf(1, 2, 3, 4), first)

        // 触发回绕
        ring.write(byteArrayOf(7, 8, 9, 10, 11, 12))
        val second = ByteArray(8)
        val read = ring.read(second)
        assertEquals(8, read)
        assertArrayEquals(byteArrayOf(5, 6, 7, 8, 9, 10, 11, 12), second)
    }

    @Test
    fun `clear 清空缓冲`() {
        val ring = PcmRingBuffer(8)
        ring.write(byteArrayOf(1, 2, 3))
        ring.clear()
        assertEquals(0, ring.readableBytes())
    }

    @Test
    fun `零长度读写返回 0`() {
        val ring = PcmRingBuffer(8)
        assertEquals(0, ring.write(ByteArray(0)))
        assertEquals(0, ring.read(ByteArray(0)))
    }
}
