package dev.beam.core.decoder

import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat

/**
 * 设备解码器枚举。
 *
 * 技术风险表：「国产机 MediaCodec 差异 → 解码器白名单 + 自动探测」。
 * 这里把设备信息抽成 [DecoderCandidate]，让选型逻辑可以脱离设备做单元测试。
 *
 * ## 为什么能力探测走反射
 *
 * `MediaCodecInfo` 的能力结构在 Android 16（API 36）发生了改名
 * （`MediaCodecInfo.Capabilities` → `MediaCodecInfo.CodecCapabilities`），
 * 且 `VideoCapabilities.getSupportedWidths()` 从 `int[]` 改成了 `Range<Integer>`。
 * 本工程 `minSdk = 26`，直接引用新类型会在旧设备上 `NoClassDefFoundError`，
 * 因此**能力探测一律用反射**，对两种 API 形态都成立。
 */
object DeviceDecoders {

    /** `MediaCodecInfo.Capabilities.FEATURE_LOW_LATENCY` 的字面量。 */
    private const val FEATURE_LOW_LATENCY = "feature.low-latency"

    /** 枚举设备上支持目标编码的解码器。 */
    fun candidates(codec: DecoderCodec): List<DecoderCandidate> {
        val mime = codec.mime
        val list = MediaCodecList(MediaCodecList.REGULAR_CODECS)
        val infos = list.codecInfos.filter { info ->
            info.supportedTypes.any { it.equals(mime, ignoreCase = true) }
        }
        return infos.map { it.toCandidate() }
    }

    /** 取出首个匹配指定名称的解码器。 */
    fun findByName(name: String): MediaCodecInfo? =
        MediaCodecList(MediaCodecList.ALL_CODECS).codecInfos.firstOrNull { it.name == name }

    private fun MediaCodecInfo.toCandidate(): DecoderCandidate {
        val vendor = vendorToken(name)
        val supported = mutableSetOf<DecoderCodec>()
        var lowLatency = false
        var maxWidth = 0

        for (type in supportedTypes) {
            when (type.lowercase()) {
                "video/avc" -> supported += DecoderCodec.H264
                "video/hevc" -> supported += DecoderCodec.HEVC
                "video/av1" -> supported += DecoderCodec.AV1
            }
            val caps = runCatching { getCapabilitiesForType(type) }.getOrNull() ?: continue
            if (probeLowLatency(caps)) lowLatency = true
            val width = probeMaxWidth(caps)
            if (width > maxWidth) maxWidth = width
        }

        return DecoderCandidate(
            name = name,
            vendor = vendor,
            hardwareAccelerated = isHardwareAccelerated,
            lowLatency = lowLatency,
            codecs = supported,
            maxWidth = maxWidth,
        )
    }

    /**
     * 从解码器名里取出厂商 token。
     *
     * `c2.qti.hevc.decoder` → `qti`；`OMX.mtk.hevc.decoder` → `mtk`；
     * `c2.android.hevc.decoder` → `android`。
     */
    internal fun vendorToken(name: String): String {
        val segments = name.lowercase().split('.', '_').filter { it.isNotBlank() }
        if (segments.isEmpty()) return ""
        val withoutPrefix = if (segments.first() == "c2" || segments.first() == "omx") {
            segments.drop(1)
        } else {
            segments
        }
        return withoutPrefix.firstOrNull() ?: segments.first()
    }

    /** 是否声明支持低延迟（`feature.low-latency`）。 */
    private fun probeLowLatency(caps: Any): Boolean = runCatching {
        val method = caps.javaClass.methods
            .firstOrNull { it.name == "getCapabilityForKey" && it.parameterCount == 1 }
            ?: return@runCatching false
        (method.invoke(caps, FEATURE_LOW_LATENCY) as? Int) == 1
    }.getOrDefault(false)

    /** 支持的最大宽度（探测不到时返回 0）。 */
    private fun probeMaxWidth(caps: Any): Int = runCatching {
        val videoCaps = caps.javaClass.methods
            .firstOrNull { it.name == "getVideoCapabilities" && it.parameterCount == 0 }
            ?.invoke(caps)
            ?: return@runCatching 0
        val widths = videoCaps.javaClass.methods
            .firstOrNull { it.name == "getSupportedWidths" && it.parameterCount == 0 }
            ?.invoke(videoCaps)
            ?: return@runCatching 0
        // android.util.Range<Integer>.getUpper()；旧 API 返回 int[] 时该调用不存在 → 0。
        widths.javaClass.methods
            .firstOrNull { it.name == "getUpper" && it.parameterCount == 0 }
            ?.invoke(widths) as? Int ?: 0
    }.getOrDefault(0)

    /** 组装解码器的低延迟参数包。 */
    fun lowLatencyParams(): android.os.Bundle = android.os.Bundle().apply {
        putInt(MediaFormat.KEY_LOW_LATENCY, 1)
    }
}
