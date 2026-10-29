package dev.beam.core.data

import dev.beam.core.net.StreamCfg
import dev.beam.core.net.VideoCodec

/**
 * 串流设置。
 *
 * 对应产品方案 §7 `:feature-settings` 的「画质、帧率、解码器、统计浮层开关」。
 * 优先级链见 §3.14：内置默认 < 设备能力推导 < 用户配置 < 会话内临时覆盖。
 */
data class BeamSettings(
    /** 画面宽度。 */
    val width: Int = 1920,
    /** 画面高度。 */
    val height: Int = 1080,
    /** 目标帧率。 */
    val fps: Int = 60,
    /** 编码格式。 */
    val codec: VideoCodec = VideoCodec.HEVC,
    /** 码率（kbps）。 */
    val bitrateKbps: Int = 20_000,
    /** FEC 冗余比例（§3.2 默认 0.2）。 */
    val fecRatio: Float = 0.2f,
    /** 是否启用音频（v1 里程碑）。 */
    val audioEnabled: Boolean = false,
    /** 是否显示串流统计浮层。 */
    val overlayEnabled: Boolean = true,
    /** 解码器偏好。 */
    val decoderPreference: DecoderPreference = DecoderPreference.AUTO,
    /** 主题模式。 */
    val themeMode: ThemeMode = ThemeMode.SYSTEM,
) {
    /** 转为送给 Rust 侧的串流配置。 */
    fun toStreamCfg(): StreamCfg = StreamCfg(
        width = width,
        height = height,
        fps = fps,
        codec = codec,
        bitrateKbps = bitrateKbps,
        fecRatio = fecRatio,
        audioEnabled = audioEnabled,
    )

    /** 分辨率展示文案，如 `1920x1080`。 */
    val resolutionLabel: String get() = "${width}x$height"
}

/** 解码器偏好。 */
enum class DecoderPreference {
    /** 自动探测（§3.12「自动探测」）。 */
    AUTO,

    /** 优先厂商 low-latency 解码器。 */
    LOW_LATENCY,

    /** 优先系统默认解码器。 */
    SYSTEM,
}

/** 主题模式。 */
enum class ThemeMode { SYSTEM, LIGHT, DARK }
