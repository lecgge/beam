package dev.beam.feature.discovery

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import dev.beam.core.net.DiscoveredHost
import dev.beam.core.net.DiscoverySource
import dev.beam.core.net.Peer
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow

/**
 * 主机扫描源。
 *
 * §3.4：`beam-discovery` 用 mDNS 广播 `_beam._udp.local`；Android 端**可选**用
 * `NsdManager` 兜底（在 Kotlin 层实现）。
 */
interface HostScanner {

    /** 扫描结果流。 */
    fun scan(): Flow<DiscoveredHost>
}

/**
 * `NsdManager` 兜底扫描。
 *
 * Rust 侧 mDNS（mdns-sd）在部分 Android 设备/网络环境下不可用（组播被阻断、
 * 应用被省电策略限制），此时用系统 `NsdManager` 兜底。
 *
 * > 注意：本实现只做**发现**，不做连接，也不做配对决策。
 */
class NsdHostScanner(
    private val context: Context,
    private val serviceType: String = SERVICE_TYPE,
) : HostScanner {

    override fun scan(): Flow<DiscoveredHost> = callbackFlow {
        val manager = context.getSystemService(Context.NSD_SERVICE) as? NsdManager
        if (manager == null) {
            close()
            return@callbackFlow
        }

        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) = Unit

            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onServiceFound(service: NsdServiceInfo) {
                resolve(manager, service) { host -> trySend(host) }
            }

            override fun onServiceLost(service: NsdServiceInfo) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                close()
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit
        }

        manager.discoverServices(serviceType, NsdManager.PROTOCOL_DNS_SD, listener)
        awaitClose {
            runCatching { manager.stopServiceDiscovery(listener) }
        }
    }

    private fun resolve(
        manager: NsdManager,
        service: NsdServiceInfo,
        onHost: (DiscoveredHost) -> Unit,
    ) {
        @Suppress("DEPRECATION")
        manager.resolveService(service, object : NsdManager.ResolveListener {
            override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) = Unit

            override fun onServiceResolved(info: NsdServiceInfo) {
                val host = info.host ?: return
                // NsdManager 的 TXT 记录是 Map<String, ByteArray>。
                val attributes = info.attributes ?: emptyMap()
                fun attr(key: String): String? =
                    attributes[key]?.toString(Charsets.UTF_8)?.takeIf { it.isNotEmpty() }
                val peer = Peer(
                    id = attr("id") ?: info.serviceName,
                    name = attr("name") ?: info.serviceName,
                    address = "${host.hostAddress}:${info.port}",
                    fingerprint = attr("fp") ?: "",
                    platform = attr("platform") ?: Peer.PLATFORM_WINDOWS,
                )
                onHost(
                    DiscoveredHost(
                        peer = peer,
                        discoveredBy = DiscoverySource.NSD,
                        lastSeenMs = System.currentTimeMillis(),
                    ),
                )
            }
        })
    }

    companion object {
        /** mDNS 服务类型（§3.4）。 */
        const val SERVICE_TYPE: String = "_beam._udp."
    }
}
