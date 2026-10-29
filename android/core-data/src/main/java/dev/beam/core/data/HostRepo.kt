package dev.beam.core.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.stringPreferencesKey
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import java.io.IOException

/**
 * 信任主机仓库（§3.12 `:core-data` 的对外接口之二）。
 *
 * 信任库以「指纹」为键：配对成功后写入 [TrustedHost]，之后同一指纹的主机
 * 直接免配对连接（§3.3 `TrustedPeer`）。
 *
 * > 安全前提（产品方案 §8）：扫码链路在局域网上是明文，**主机端必须弹窗人工
 * > 确认后才会写入信任库**。客户端侧的写入只发生在用户主动完成配对之后。
 */
class HostRepo(
    private val dataStore: DataStore<Preferences>,
) {

    /** 已信任的主机列表。 */
    val hosts: Flow<List<TrustedHost>> = dataStore.data
        .catch { cause ->
            if (cause is IOException) emit(emptyPreferences()) else throw cause
        }
        .map { prefs -> PeerCodec.decodeAll(prefs[Keys.TRUSTED_HOSTS] ?: "") }

    /** 新增或更新一条信任记录（按指纹去重）。 */
    suspend fun upsert(host: TrustedHost) {
        dataStore.edit { prefs ->
            val current = PeerCodec.decodeAll(prefs[Keys.TRUSTED_HOSTS] ?: "")
            val merged = current.filterNot { it.peer.fingerprint == host.peer.fingerprint } + host
            prefs[Keys.TRUSTED_HOSTS] = PeerCodec.encodeAll(merged)
        }
    }

    /** 按指纹移除信任记录。 */
    suspend fun remove(fingerprint: String) {
        dataStore.edit { prefs ->
            val current = PeerCodec.decodeAll(prefs[Keys.TRUSTED_HOSTS] ?: "")
            prefs[Keys.TRUSTED_HOSTS] = PeerCodec.encodeAll(
                current.filterNot { it.peer.fingerprint == fingerprint },
            )
        }
    }

    /** 按指纹查找信任记录。 */
    suspend fun find(fingerprint: String): TrustedHost? =
        hosts.first().firstOrNull { it.peer.fingerprint == fingerprint }

    /** 指纹是否已被信任。 */
    suspend fun isTrusted(fingerprint: String): Boolean = find(fingerprint) != null

    /** 清空信任库。 */
    suspend fun clear() {
        dataStore.edit { prefs -> prefs[Keys.TRUSTED_HOSTS] = "" }
    }

    private object Keys {
        val TRUSTED_HOSTS = stringPreferencesKey("trusted_hosts")
    }
}
