package dev.beam.core.decoder

/**
 * 解码器工厂（§3.12 `:core-decoder` 对外接口的补充）。
 *
 * ## 为什么要有这个工厂
 *
 * §3.12 明确禁止 `:feature-stream` 接触**解码细节**：
 * 解码器应当由 `:core-decoder` 侧以**工厂 / `VideoFormat` 构造器**下沉。
 * 本工厂把「编码名 → 解码器 / 流格式」的映射全部收在解码器模块里，
 * 上层只需要传一个**字符串形式的编码名**（如 `"H264"`），不需要知道
 * `MediaCodec` / `DecoderCodec` / `VideoFormat` 的任何细节。
 */
object VideoDecoderFactory {

    /**
     * 按编码名解析容器格式。
     *
     * 名字不认识时回退到 [DecoderCodec.HEVC]（与主机端默认一致）。
     */
    fun codecOf(codecName: String): DecoderCodec = when (codecName.trim().lowercase()) {
        "h264", "h.264", "avc" -> DecoderCodec.H264
        "av1" -> DecoderCodec.AV1
        else -> DecoderCodec.HEVC
    }

    /**
     * 创建解码器。
     *
     * @param codecName 编码名（`H264` / `HEVC` / `AV1`，大小写不敏感）。
     * @param preferLowLatency 是否优选厂商 low_latency 解码器。
     * @param preferredDecoderName 强制指定解码器名；为 null 时自动探测。
     */
    fun create(
        codecName: String,
        preferLowLatency: Boolean = true,
        preferredDecoderName: String? = null,
    ): VideoDecoder = MediaCodecVideoDecoder(
        preferredDecoderName = preferredDecoderName,
        preferLowLatency = preferLowLatency,
    )

    /**
     * 构造流格式。
     *
     * @param width 画面宽（像素）。
     * @param height 画面高（像素）。
     * @param codecName 编码名。
     * @param fps 目标帧率。
     */
    fun format(width: Int, height: Int, codecName: String, fps: Int): VideoFormat = VideoFormat(
        width = width,
        height = height,
        codec = codecOf(codecName),
        fps = fps,
    )
}
