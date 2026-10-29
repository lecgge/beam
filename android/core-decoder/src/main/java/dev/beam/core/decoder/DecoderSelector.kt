package dev.beam.core.decoder

/**
 * 解码器候选（厂商 + 名称）。
 *
 * 抽象成纯数据是为了让**选型逻辑可以单元测试**：真实的 `MediaCodecInfo`
 * 在 JVM 单元测试里不可用。
 */
data class DecoderCandidate(
    /** `MediaCodec.getName()`，形如 `c2.qti.hevc.decoder` / `OMX.google.h264.decoder`。 */
    val name: String,
    /** 厂商前缀（`name` 的第一段），如 `qti` / `c2` / `OMX` / `google`。 */
    val vendor: String,
    /** 是否为硬件解码器（软解在 4K60 下不可能达标）。 */
    val hardwareAccelerated: Boolean,
    /** 是否声明支持低延迟（`MediaFormat.KEY_LOW_LATENCY`）。 */
    val lowLatency: Boolean,
    /** 支持的编码格式。 */
    val codecs: Set<DecoderCodec>,
    /** 最大宽度（0 表示未知）。 */
    val maxWidth: Int = 0,
) {
    /** 厂商名小写，便于做前缀匹配。 */
    val vendorLower: String get() = vendor.lowercase()
}

/**
 * 解码器选型。
 *
 * §3.12 要求「厂商 low_latency 解码器」优先；技术风险表也提到「国产机
 * MediaCodec 差异 → 解码器白名单 + 自动探测」。
 *
 * 优先级：
 * 1. 硬解 + 支持目标编码 + 声明低延迟 + 厂商在低延迟白名单内；
 * 2. 硬解 + 支持目标编码；
 * 3. 软解（仅在无硬解时兜底）。
 */
object DecoderSelector {

    /**
     * 已知的低延迟解码器厂商白名单（Android 厂商硬解前缀）。
     *
     * 命中白名单的解码器在同等条件下优先选用。
     */
    val LOW_LATENCY_VENDORS: List<String> = listOf(
        "qti",     // 高通
        "qcom",    // 高通（旧命名）
        "mtk",     // 联发科
        "exynos",  // 三星
        "hisilicon", "hisi", // 华为海思
        "amlogic", // 晶晨（TV）
        "rk",      // 瑞芯微（TV）
        "marlin",  // 联发科 TV
    )

    /**
     * 在候选列表中挑选最合适的解码器。
     *
     * @param candidates 设备上枚举到的解码器。
     * @param codec      目标编码格式。
     * @param preferLowLatency 是否优先低延迟解码器（默认 true）。
     * @return 选中的解码器；没有可用项时返回 null。
     */
    fun select(
        candidates: List<DecoderCandidate>,
        codec: DecoderCodec,
        preferLowLatency: Boolean = true,
    ): DecoderCandidate? {
        val supported = candidates.filter { it.codecs.contains(codec) }
        if (supported.isEmpty()) return null

        val hardware = supported.filter { it.hardwareAccelerated }
        val pool = if (hardware.isNotEmpty()) hardware else supported

        if (preferLowLatency) {
            val lowLatency = pool.filter { it.lowLatency }
            if (lowLatency.isNotEmpty()) {
                val vendorPreferred = lowLatency.filter { isKnownLowLatencyVendor(it.name) }
                if (vendorPreferred.isNotEmpty()) return vendorPreferred.first()
                return lowLatency.first()
            }
        }

        val vendorPreferred = pool.filter { isKnownLowLatencyVendor(it.name) }
        if (vendorPreferred.isNotEmpty()) return vendorPreferred.first()
        return pool.first()
    }

    /**
     * 判断解码器名是否属于低延迟白名单厂商。
     *
     * 解码器名的厂商 token 不一定在开头：`c2.qti.hevc.decoder` 的厂商段是
     * `qti`（第二段），`OMX.mtk.hevc.decoder` 的厂商段是 `mtk`（第二段），
     * 因此这里按**点分段精确匹配**（允许前缀，如 `hisi` 匹配 `hisilicon`）。
     */
    fun isKnownLowLatencyVendor(name: String): Boolean {
        val segments = name.lowercase().split('.', '_')
        return LOW_LATENCY_VENDORS.any { vendor ->
            segments.any { it == vendor || it.startsWith(vendor) }
        }
    }
}
