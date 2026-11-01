package dev.beam.core.net.fakes

/**
 * 与 uniffi 生成物**同构**的替身类型。
 *
 * ## 用途
 *
 * uniffi 生成物（`uniffi.beam_ffi.*`）是构建产物，不随源码入库；
 * 而且它依赖 JNA 与 `libbeam_ffi.so`，在 JVM 单测里加载不了。
 * 这些替身让 [dev.beam.core.net.NativeMappings] 的反射映射逻辑可以被单测覆盖。
 *
 * ## 形状来源
 *
 * 取自 `crates/beam-ffi/src/ffi.rs` 的 uniffi 导出面：
 * - 枚举变体是**嵌套**在 sealed class 里的 data class（Kotlin 里 `simpleName` 就是变体名）；
 * - record 的数值字段用 Kotlin 的无符号整型（`UInt` / `ULong`），
 *   因此它们的 getter 在 JVM 上带 inline class 修饰后缀（`getFps-<hash>`）。
 */
sealed class SessionEvent {

    /** 状态变化。 */
    data class StateChanged(val state: SessionState) : SessionEvent()

    /** 能力协商完成。 */
    data class Negotiated(val config: StreamCfg) : SessionEvent()

    /** 统计上报（统计字段在**嵌套**记录上）。 */
    data class Stats(val stats: SessionStats) : SessionEvent()

    /** 出错。 */
    data class Error(val code: String, val message: String, val retryable: Boolean) : SessionEvent()

    /** 会话结束。 */
    object Closed : SessionEvent()

    /**
     * 丢帧（§3.2 E2004）。
     *
     * 数值字段用无符号整型，与 uniffi 生成物同构（`count: UInt` / `code: UShort`）。
     */
    data class FramesDropped(val count: kotlin.UInt, val code: kotlin.UShort) : SessionEvent()
}

/** 会话状态。 */
enum class SessionState {
    Idle,
    Discovering,
    Pairing,
    Connecting,
    Negotiating,
    Streaming,
    Reconnecting,
    Closed,
    Error,
}

/** 统计上报（数值字段是无符号整型）。 */
data class SessionStats(
    val sessionId: kotlin.ULong,
    val fps: kotlin.UInt,
    val bitrateBps: kotlin.UInt,
    val lossPpm: kotlin.UInt,
    val rttUs: kotlin.UInt,
    val decodeUs: kotlin.UInt,
    val renderUs: kotlin.UInt,
    val e2eUs: kotlin.UInt,
    val droppedFrames: kotlin.UInt,
)

/** 串流配置。 */
data class StreamCfg(
    val width: kotlin.Int,
    val height: kotlin.Int,
    val fps: kotlin.Int,
    val codec: String,
    val bitrateKbps: kotlin.Int,
    val fecRatio: kotlin.Float,
    val audioEnabled: kotlin.Boolean,
)

/** 发现到的主机（地址与端口是**分开的两个字段**）。 */
data class DiscoveredHost(
    val id: String,
    val name: String,
    val version: String,
    val fingerprint: String,
    val port: kotlin.UInt,
    val address: String,
    val fullname: String,
    /** 平台（§3.4 mDNS TXT 的 `plat`）。 */
    val platform: String,
)

/** 主机信息。 */
data class Peer(
    val id: String,
    val name: String,
    val address: String,
    val fingerprint: String,
    val platform: String,
    val trusted: kotlin.Boolean,
)

/**
 * 重连信息（§4.1）。
 *
 * 由 uniffi 的 `Session::reconnecting()` **查询**得到，不在重连态时返回 `null`。
 */
data class ReconnectingInfo(
    val elapsedMs: kotlin.ULong,
    val keepLastFrame: kotlin.Boolean,
)

/** 会话汇总（§3.9）。 */
data class SessionSummary(
    val sessionId: kotlin.ULong,
    val durationMs: kotlin.ULong,
    val frames: kotlin.ULong,
    val droppedFrames: kotlin.ULong,
    val reconnects: kotlin.UInt,
    val rttUs: kotlin.UInt,
    val lossPpm: kotlin.UInt,
    val fps: kotlin.UInt,
    val bitrateBps: kotlin.UInt,
    val state: SessionState,
)
