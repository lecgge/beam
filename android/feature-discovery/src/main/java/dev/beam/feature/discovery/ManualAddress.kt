package dev.beam.feature.discovery

import dev.beam.core.net.Peer

/**
 * 手动输入的主机地址解析（空态「手动输入 IP」动作）。
 *
 * 纯函数，无 Android 依赖，方便单元测试。
 */
object ManualAddress {

    /** 主机缺省端口（与 Windows 主机端一致）。 */
    const val DEFAULT_PORT: Int = 4827

    /** IPv4 地址格式。 */
    private val IP_PATTERN = Regex("^\\d{1,3}(\\.\\d{1,3}){3}$")

    /**
     * 解析用户输入的地址。
     *
     * 接受 `ip` 与 `ip:port` 两种写法，缺省端口 [DEFAULT_PORT]。
     *
     * @return 合法时返回主机对象（未信任、指纹为空，配对成功后由 Rust 侧带回指纹）；
     *   非法时返回 null，由调用方在页面上提示错误。
     */
    fun parse(input: String): Peer? {
        val trimmed = input.trim()
        if (trimmed.isEmpty()) return null
        val host = trimmed.substringBefore(':')
        val port = trimmed.substringAfter(':', DEFAULT_PORT.toString()).toIntOrNull() ?: return null
        if (!IP_PATTERN.matches(host)) return null
        if (port !in 1..65535) return null
        val address = "$host:$port"
        return Peer(
            id = address,
            name = address,
            address = address,
            // 手动输入的主机没有指纹；配对成功后由 Rust 侧带回固定指纹。
            fingerprint = "",
            trusted = false,
        )
    }
}
