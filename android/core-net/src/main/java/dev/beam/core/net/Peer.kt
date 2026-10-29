package dev.beam.core.net

/**
 * 远端主机（Windows 主机端）。
 *
 * 字段与 `beam-proto` / `beam-pair` 的主机信息对齐：
 *  - [id] 主机稳定标识（mDNS TXT 中的 `id`）
 *  - [name] 展示名
 *  - [address] `ip:port`
 *  - [fingerprint] 证书指纹（十六进制），配对后固定不变
 *  - [platform] 主机平台标识，Android 端当前仅消费 `windows`
 *  - [trusted] 本地信任库中是否已存在该指纹
 */
data class Peer(
    val id: String,
    val name: String,
    val address: String,
    val fingerprint: String,
    val platform: String = PLATFORM_WINDOWS,
    val trusted: Boolean = false,
) {
    val host: String get() = address.substringBefore(':', missingDelimiterValue = "")
    val port: Int get() = address.substringAfter(':', missingDelimiterValue = "").toIntOrNull() ?: 0

    companion object {
        const val PLATFORM_WINDOWS = "windows"
    }
}

/** 局域网发现到的主机条目（尚未配对也可能出现）。 */
data class DiscoveredHost(
    val peer: Peer,
    val discoveredBy: DiscoverySource,
    val lastSeenMs: Long,
)

/** 发现来源：Rust 侧 mDNS 为主，Kotlin 侧 NsdManager 为兜底。 */
enum class DiscoverySource {
    /** Rust `beam-discovery`(mdns-sd) 经 uniffi 回调上送。 */
    MDNS,

    /** Android `NsdManager` 兜底扫描。 */
    NSD,
}
