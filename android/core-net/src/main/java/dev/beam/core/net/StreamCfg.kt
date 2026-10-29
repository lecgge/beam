package dev.beam.core.net

/** 视频编码格式，与主机端 `beam-encode` 的 codec 取值一致。 */
enum class VideoCodec {
    H264,
    HEVC,
    AV1,
}

/**
 * 串流配置。
 *
 * 与 `beam-proto::StreamConfig` 语义对齐：分辨率/帧率/编码/码率/FEC 比例。
 * 字段保持可空默认值，便于在能力协商前使用保守值。
 */
data class StreamCfg(
    val width: Int = 1920,
    val height: Int = 1080,
    val fps: Int = 60,
    val codec: VideoCodec = VideoCodec.HEVC,
    val bitrateKbps: Int = 20_000,
    val fecRatio: Float = 0.2f,
    val audioEnabled: Boolean = false,
) {
    init {
        require(width > 0 && height > 0) { "分辨率必须为正数" }
        require(fps > 0) { "帧率必须为正数" }
        require(fecRatio in 0f..1f) { "FEC 比例必须落在 0..1" }
    }

    /** 画质档位，仅用于 UI 展示与日志。 */
    val tier: Tier
        get() = when {
            width >= 3840 -> Tier.T1
            width >= 2560 -> Tier.T2
            else -> Tier.T0
        }
}

/** 画质档位，对应技术设计文档 §8 的 T0/T1/T2。 */
enum class Tier { T0, T1, T2 }
