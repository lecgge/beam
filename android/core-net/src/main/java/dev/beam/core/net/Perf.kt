package dev.beam.core.net

/**
 * `perf://tick`（debug，250 ms）事件负载。
 *
 * §3.11 的事件清单里 `perf://tick` 只在 debug 构建下产生；Rust 侧对应
 * `beam-perf`（cargo feature `perf-stats`，默认关闭）。
 *
 * 本类型放在 `:core-net` 而不是 `:feature-perf`，是为了让 `:feature-perf`
 * 只在 debug 被依赖（§3.12 / §3.13 的 release 门禁），同时让 `:core-net`
 * 的控制面事件模型保持自洽。
 */
data class PerfTick(
    /** 各阶段耗时（毫秒）：`capture` / `encode` / `packetize` / `network` / `decode` / `render` 等。 */
    val stagesMs: Map<String, Double> = emptyMap(),
    /** 当前帧率。 */
    val fps: Double = 0.0,
    /** 当前码率（kbps）。 */
    val bitrateKbps: Double = 0.0,
    /** 累计掉帧数。 */
    val droppedFrames: Long = 0L,
    /** 端到端延迟（毫秒），若主机侧已做时钟同步则有效。 */
    val e2eMs: Double = 0.0,
)

/**
 * `perf://tick` 事件源。
 *
 * Rust 侧仅在 `perf-stats` 开启时提供该流；release 构建拿到的实现通常直接
 * 返回空流。`:feature-perf` 通过本接口消费 debug 统计。
 */
interface PerfTickSource {

    /** `perf://tick` 事件流（debug 250 ms）。 */
    fun perfTicks(): kotlinx.coroutines.flow.Flow<PerfTick>
}
