package dev.beam.core.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

/**
 * 基于会话统计的 [PerfTickSource] 实现（§3.13）。
 *
 * ## 数据来源
 *
 * **真实数据**：`session://stats`（500 ms 一次）的 [SessionStats]，也就是
 * Rust 侧下发的客户端可观测统计。本实现只做**字段搬运与阶段拆分**，
 * **不产生任何合成值**：没有采样就没有数据，面板显示空态。
 *
 * ## 阶段拆分
 *
 * `SessionStats` 里与「阶段耗时」同义的字段被拆成 §3.13 的阶段名：
 * `decode`（解码）、`render`（上屏）、`network`（RTT）、`e2e`（端到端）。
 * 取值为 0 的字段**不写入**（0 代表「没有采样」，不是「耗时 0 ms」）。
 */
class SessionStatsTickSource(
    private val stats: Flow<SessionStats>,
) : PerfTickSource {

    /** `session://stats` → `perf://tick` 负载。 */
    override fun perfTicks(): Flow<PerfTick> = stats.map { it.toPerfTick() }

    private fun SessionStats.toPerfTick(): PerfTick = PerfTick(
        stagesMs = buildMap {
            decodeMs.takeIf { it > 0.0 }?.let { put(STAGE_DECODE, it) }
            renderMs.takeIf { it > 0.0 }?.let { put(STAGE_RENDER, it) }
            rttMs.takeIf { it > 0.0 }?.let { put(STAGE_NETWORK, it) }
            e2eMs.takeIf { it > 0.0 }?.let { put(STAGE_E2E, it) }
        },
        fps = fps,
        bitrateKbps = bitrateKbps,
        droppedFrames = droppedFrames,
        e2eMs = e2eMs,
    )

    companion object {
        /** 解码阶段。 */
        const val STAGE_DECODE: String = "decode"

        /** 上屏阶段。 */
        const val STAGE_RENDER: String = "render"

        /** 网络往返阶段（RTT）。 */
        const val STAGE_NETWORK: String = "network"

        /** 端到端阶段。 */
        const val STAGE_E2E: String = "e2e"
    }
}
