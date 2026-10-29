package dev.beam.core.data

import dev.beam.core.net.Peer
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [SettingsRepo] / [HostRepo] 单元测试。
 *
 * 使用内存版 DataStore（见 [InMemoryPreferenceDataStore] 的说明：Windows 上
 * `File.renameTo` 不能覆盖已存在文件，真实文件 DataStore 无法连续写入）。
 */
class SettingsRepoTest {

    private fun settingsRepo() = SettingsRepo(InMemoryPreferenceDataStore())

    private fun hostRepo() = HostRepo(InMemoryPreferenceDataStore())

    @Test
    fun `默认值来自 BeamSettings`() = runTest {
        val repo = settingsRepo()
        assertEquals(BeamSettings(), repo.settings.first())
    }

    @Test
    fun `更新设置会被持久化`() = runTest {
        val repo = settingsRepo()
        repo.update { it.copy(width = 2560, height = 1440, fps = 120) }
        val updated = repo.settings.first()
        assertEquals(2560, updated.width)
        assertEquals(1440, updated.height)
        assertEquals(120, updated.fps)
    }

    @Test
    fun `reset 恢复默认值`() = runTest {
        val repo = settingsRepo()
        repo.update { it.copy(width = 3840, height = 2160) }
        repo.reset()
        assertEquals(BeamSettings(), repo.settings.first())
    }

    @Test
    fun `未知枚举值回退到默认值`() = runTest {
        val store = InMemoryPreferenceDataStore()
        val repo = SettingsRepo(store)
        repo.update { it.copy(codec = dev.beam.core.net.VideoCodec.AV1) }
        assertEquals(dev.beam.core.net.VideoCodec.AV1, repo.settings.first().codec)
    }

    @Test
    fun `设置可转换为串流配置`() {
        val cfg = BeamSettings(width = 1920, height = 1080, fps = 60).toStreamCfg()
        assertEquals(1920, cfg.width)
        assertEquals(60, cfg.fps)
    }

    @Test
    fun `信任主机可写入并按指纹查找`() = runTest {
        val repo = hostRepo()
        val peer = Peer(
            id = "desktop",
            name = "DESKTOP",
            address = "192.168.1.23:47989",
            fingerprint = "9F2AC0D1",
        )
        assertFalse(repo.isTrusted(peer.fingerprint))
        repo.upsert(TrustedHost(peer = peer, pairedAtMs = 1L))
        assertTrue(repo.isTrusted(peer.fingerprint))
        // 解码出来的记录固定标记为已信任。
        assertEquals(peer.copy(trusted = true), repo.find(peer.fingerprint)?.peer)
    }

    @Test
    fun `同一指纹重复写入不会产生重复记录`() = runTest {
        val repo = hostRepo()
        val peer = Peer(id = "a", name = "A", address = "1.2.3.4:1", fingerprint = "FP")
        repo.upsert(TrustedHost(peer = peer))
        repo.upsert(TrustedHost(peer = peer.copy(name = "A2")))
        val hosts = repo.hosts.first()
        assertEquals(1, hosts.size)
        assertEquals("A2", hosts.first().peer.name)
    }

    @Test
    fun `移除信任记录`() = runTest {
        val repo = hostRepo()
        val peer = Peer(id = "a", name = "A", address = "1.2.3.4:1", fingerprint = "FP")
        repo.upsert(TrustedHost(peer = peer))
        repo.remove(peer.fingerprint)
        assertNull(repo.find(peer.fingerprint))
    }
}
