package dev.beam.feature.pairing

/**
 * 扫码载荷解析（§3.3）。
 *
 * 载荷形如 `beam://pair?h=ip:port&fp=<指纹>&t=<token>`：
 * - `h`：主机地址 `ip:port`；
 * - `fp`：证书指纹（十六进制）；
 * - `t`：一次性 token（120 秒有效）。
 */
object PairPayload {

    private const val SCHEME = "beam://pair?"

    /** 解析结果。 */
    data class Parsed(
        val host: String,
        val port: Int,
        val fingerprint: String,
        val token: String,
    ) {
        /** `ip:port`。 */
        val address: String get() = "$host:$port"
    }

    /**
     * 解析扫码载荷。
     *
     * @return 载荷非法（协议不符 / 缺字段 / 端口非法）时返回 null。
     */
    fun parse(payload: String): Parsed? {
        val trimmed = payload.trim()
        if (!trimmed.startsWith(SCHEME)) return null
        val query = trimmed.removePrefix(SCHEME)
        val params = query.split('&')
            .mapNotNull { part ->
                val idx = part.indexOf('=')
                if (idx <= 0) null else part.substring(0, idx) to part.substring(idx + 1)
            }
            .toMap()
        val address = params["h"] ?: return null
        val fingerprint = params["fp"] ?: return null
        val token = params["t"] ?: return null
        val separator = address.lastIndexOf(':')
        if (separator <= 0) return null
        val host = address.substring(0, separator)
        val port = address.substring(separator + 1).toIntOrNull() ?: return null
        if (host.isBlank() || port !in 1..65535) return null
        return Parsed(host = host, port = port, fingerprint = fingerprint, token = token)
    }
}
