package dev.beam.core.data

import dev.beam.core.net.Peer

/**
 * 已信任的主机（§3.12 `:core-data` 的「信任主机」）。
 *
 * @param peer 主机信息（含固定指纹）。
 * @param pairedAtMs 配对时刻（墙钟毫秒）。
 * @param lastConnectedMs 最近一次连接时刻（墙钟毫秒）。
 */
data class TrustedHost(
    val peer: Peer,
    val pairedAtMs: Long = 0L,
    val lastConnectedMs: Long = 0L,
)

/**
 * 主机记录编解码。
 *
 * 刻意**不使用 JSON**：`org.json` 在 Android 单元测试里不可用，而这里的编解码
 * 需要被单元测试覆盖。改用 ASCII 单元分隔符（`0x1F`）+ 转义，对任意文本安全。
 *
 * 字段顺序：`id`、`name`、`address`、`fingerprint`、`platform`、`pairedAtMs`、`lastConnectedMs`。
 */
object PeerCodec {

    /** 字段分隔符。 */
    private const val SEP = '\u001F'

    /** 转义前缀。 */
    private const val ESC = '\u001E'

    /** 字段个数。 */
    private const val FIELD_COUNT = 7

    /** 编码成单行记录。 */
    fun encode(host: TrustedHost): String {
        val peer = host.peer
        return listOf(
            peer.id,
            peer.name,
            peer.address,
            peer.fingerprint,
            peer.platform,
            host.pairedAtMs.toString(),
            host.lastConnectedMs.toString(),
        ).joinToString(SEP.toString()) { escape(it) }
    }

    /** 解码单行记录；格式非法返回 null。 */
    fun decode(line: String): TrustedHost? {
        val parts = line.split(SEP)
        if (parts.size != FIELD_COUNT) return null
        val pairedAt = parts[5].toLongOrNull() ?: return null
        val lastConnected = parts[6].toLongOrNull() ?: return null
        return TrustedHost(
            peer = Peer(
                id = unescape(parts[0]),
                name = unescape(parts[1]),
                address = unescape(parts[2]),
                fingerprint = unescape(parts[3]),
                platform = unescape(parts[4]),
                trusted = true,
            ),
            pairedAtMs = pairedAt,
            lastConnectedMs = lastConnected,
        )
    }

    /** 编码整个信任库（每行一条记录）。 */
    fun encodeAll(hosts: List<TrustedHost>): String =
        hosts.joinToString("\n") { encode(it) }

    /** 解码整个信任库；非法行被跳过。 */
    fun decodeAll(text: String): List<TrustedHost> =
        text.lineSequence()
            .filter { it.isNotBlank() }
            .mapNotNull { decode(it.trim()) }
            .toList()

    /** 转义分隔符与转义前缀本身。 */
    private fun escape(value: String): String = buildString {
        for (c in value) {
            when (c) {
                ESC -> append(ESC).append(ESC)
                SEP -> append(ESC).append('S')
                else -> append(c)
            }
        }
    }

    /** 反转义。 */
    private fun unescape(value: String): String = buildString {
        var i = 0
        while (i < value.length) {
            val c = value[i]
            if (c == ESC && i + 1 < value.length) {
                when (value[i + 1]) {
                    ESC -> {
                        append(ESC)
                        i += 2
                    }
                    'S' -> {
                        append(SEP)
                        i += 2
                    }
                    else -> {
                        append(c)
                        i += 1
                    }
                }
            } else {
                append(c)
                i += 1
            }
        }
    }
}
