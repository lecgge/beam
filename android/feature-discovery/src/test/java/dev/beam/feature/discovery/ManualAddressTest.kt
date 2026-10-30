package dev.beam.feature.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 手动输入主机地址的解析测试（空态「手动输入 IP」动作）。
 */
class ManualAddressTest {

    @Test
    fun `只填 ip 时使用缺省端口`() {
        val peer = ManualAddress.parse("192.168.1.10")
        assertNotNull(peer)
        assertEquals("192.168.1.10:4827", peer?.address)
        assertEquals("192.168.1.10:4827", peer?.id)
        assertEquals("", peer?.fingerprint)
        assertEquals(false, peer?.trusted)
    }

    @Test
    fun `填 ip 与端口时使用指定端口`() {
        val peer = ManualAddress.parse("10.0.0.5:9000")
        assertEquals("10.0.0.5:9000", peer?.address)
    }

    @Test
    fun `输入两侧的空格会被忽略`() {
        assertEquals("10.0.0.5:4827", ManualAddress.parse("  10.0.0.5  ")?.address)
    }

    @Test
    fun `非法地址一律拒绝`() {
        assertNull(ManualAddress.parse(""))
        assertNull(ManualAddress.parse("   "))
        assertNull(ManualAddress.parse("192.168.1"))
        assertNull(ManualAddress.parse("not-an-ip"))
        assertNull(ManualAddress.parse("192.168.1.10:"))
        assertNull(ManualAddress.parse("192.168.1.10:0"))
        assertNull(ManualAddress.parse("192.168.1.10:70000"))
        assertNull(ManualAddress.parse("192.168.1.10:abc"))
    }
}
