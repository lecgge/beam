package dev.beam.core.data

import dev.beam.core.net.Peer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [PeerCodec] 单元测试：信任主机的持久化编解码。
 */
class PeerCodecTest {

    private val host = TrustedHost(
        peer = Peer(
            id = "desktop-leyu",
            name = "DESKTOP-LEYU",
            address = "192.168.1.23:47989",
            fingerprint = "9F2AC0D1",
            platform = Peer.PLATFORM_WINDOWS,
        ),
        pairedAtMs = 1_700_000_000_000L,
        lastConnectedMs = 1_700_000_500_000L,
    )

    @Test
    fun `编解码往返一致`() {
        val decoded = PeerCodec.decode(PeerCodec.encode(host))
        // 解码出来的是「已信任」记录，因此 trusted 固定为 true。
        assertEquals(host.copy(peer = host.peer.copy(trusted = true)), decoded)
    }

    @Test
    fun `名称含分隔符也不会破坏字段`() {
        val tricky = host.copy(peer = host.peer.copy(name = "客厅\u001F电视"))
        val decoded = PeerCodec.decode(PeerCodec.encode(tricky))
        assertEquals("客厅\u001F电视", decoded?.peer?.name)
    }

    @Test
    fun `非法记录返回 null`() {
        assertNull(PeerCodec.decode(""))
        assertNull(PeerCodec.decode("a\u001Fb\u001Fc"))
        assertNull(PeerCodec.decode("a\u001Fb\u001Fc\u001Fd\u001Fe\u001Fnotanumber\u001Fg"))
    }

    @Test
    fun `信任库整体编解码`() {
        val hosts = listOf(host, host.copy(peer = host.peer.copy(id = "second", fingerprint = "AA")))
        val decoded = PeerCodec.decodeAll(PeerCodec.encodeAll(hosts))
        assertEquals(2, decoded.size)
        assertEquals(hosts.map { it.peer.id }, decoded.map { it.peer.id })
        assertTrue(decoded.all { it.peer.trusted })
    }

    @Test
    fun `信任库解码会跳过非法行`() {
        val text = PeerCodec.encode(host) + "\nnot-a-record"
        val decoded = PeerCodec.decodeAll(text)
        assertEquals(1, decoded.size)
    }
}
