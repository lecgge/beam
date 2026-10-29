package dev.beam.feature.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * [PairPayload] 单元测试：扫码载荷解析（§3.3）。
 */
class PairPayloadTest {

    @Test
    fun `解析合法载荷`() {
        val parsed = PairPayload.parse("beam://pair?h=192.168.1.80:47989&fp=9F2AC0D1&t=tok123")
        assertEquals("192.168.1.80", parsed?.host)
        assertEquals(47989, parsed?.port)
        assertEquals("9F2AC0D1", parsed?.fingerprint)
        assertEquals("tok123", parsed?.token)
        assertEquals("192.168.1.80:47989", parsed?.address)
    }

    @Test
    fun `协议不符返回 null`() {
        assertNull(PairPayload.parse("https://example.com"))
        assertNull(PairPayload.parse(""))
    }

    @Test
    fun `缺字段返回 null`() {
        assertNull(PairPayload.parse("beam://pair?h=192.168.1.80:47989"))
        assertNull(PairPayload.parse("beam://pair?fp=9F2A&t=tok"))
    }

    @Test
    fun `端口非法返回 null`() {
        assertNull(PairPayload.parse("beam://pair?h=192.168.1.80:0&fp=9F2A&t=tok"))
        assertNull(PairPayload.parse("beam://pair?h=192.168.1.80:abc&fp=9F2A&t=tok"))
        assertNull(PairPayload.parse("beam://pair?h=192.168.1.80&fp=9F2A&t=tok"))
    }

    @Test
    fun `前后空白被忽略`() {
        val parsed = PairPayload.parse("  beam://pair?h=10.0.0.2:1234&fp=AA&t=bb  ")
        assertEquals("10.0.0.2", parsed?.host)
        assertEquals(1234, parsed?.port)
    }
}
