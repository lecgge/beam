package dev.beam.feature.perf

import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfSnapshotStore
import dev.beam.core.data.perf.PerfUiState
import dev.beam.core.net.PerfTick
import dev.beam.core.net.SessionStats
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.launch

/**
 * 性能采集器（§3.13）。
 *
 * 消费两类数据源：
 * - `session://stats`（500 ms）：客户端可观测的统计；
 * - `perf://tick`（debug，250 ms）：Rust 侧各阶段耗时明细。
 *
 * 采集结果写入 [PerfSnapshotStore]，由 `:app` 的 debug 变体展示。
 *
 * > 本模块**只**存在于 debug 变体（`:app` 用 `debugImplementation` 引入），
 * > release 变体的依赖图里没有它，因此 release APK 中不存在任何 perf 类。
 */
class PerfRecorder(
    private val scope: CoroutineScope,
    private val stats: Flow<SessionStats>,
    private val ticks: Flow<PerfTick> = emptyFlow(),
    private val accumulator: StageAccumulator = StageAccumulator(),
) {

    private var statsJob: Job? = null
    private var tickJob: Job? = null
    private val e2eHistory = ArrayDeque<Double>()

    /** 开始采集。 */
    fun start() {
        stop()
        statsJob = stats
            .onEach { snapshot -> onStats(snapshot) }
            .launchIn(scope)
        tickJob = ticks
            .onEach { tick -> onTick(tick) }
            .launchIn(scope)
    }

    /** 停止采集。 */
    fun stop() {
        statsJob?.cancel()
        tickJob?.cancel()
        statsJob = null
        tickJob = null
    }

    /** 重置历史。 */
    fun reset() {
        accumulator.clear()
        e2eHistory.clear()
        PerfSnapshotStore.reset()
    }

    /**
     * 导出当前快照。
     *
     * @param format 导出格式（§3.13 `Perf::export(fmt, path)`）。
     * @return 导出内容（文本）。
     */
    fun export(format: PerfExportFormat): String {
        val state = PerfSnapshotStore.state.value
        return when (format) {
            PerfExportFormat.CSV -> toCsv(state)
            PerfExportFormat.JSON -> toJson(state)
        }
    }

    private fun onStats(snapshot: SessionStats) {
        if (snapshot.e2eMs > 0) {
            e2eHistory.addLast(snapshot.e2eMs)
            while (e2eHistory.size > HISTORY_SIZE) e2eHistory.removeFirst()
        }
        PerfSnapshotStore.publish(
            PerfUiState(
                fps = snapshot.fps,
                bitrateKbps = snapshot.bitrateKbps,
                lossPercent = snapshot.lossPercent,
                fecRecoveredPercent = snapshot.fecRecoveredPercent,
                droppedFrames = snapshot.droppedFrames,
                rttMs = snapshot.rttMs,
                e2eMs = snapshot.e2eMs,
                resolution = "${snapshot.width}x${snapshot.height}",
                codec = snapshot.codec.name,
                stages = accumulator.timings(),
                e2eHistory = e2eHistory.toList(),
            ),
        )
    }

    private fun onTick(tick: PerfTick) {
        tick.stagesMs.forEach { (stage, ms) -> accumulator.record(stage, ms) }
    }

    private fun toCsv(state: PerfUiState): String = buildString {
        appendLine("stage,mean_ms,p99_ms")
        state.stages.forEach { appendLine("${it.stage},${it.meanMs},${it.p99Ms}") }
    }

    private fun toJson(state: PerfUiState): String = buildString {
        appendLine("{")
        appendLine("  \"fps\": ${state.fps},")
        appendLine("  \"bitrateKbps\": ${state.bitrateKbps},")
        appendLine("  \"lossPercent\": ${state.lossPercent},")
        appendLine("  \"fecRecoveredPercent\": ${state.fecRecoveredPercent},")
        appendLine("  \"droppedFrames\": ${state.droppedFrames},")
        appendLine("  \"rttMs\": ${state.rttMs},")
        appendLine("  \"e2eMs\": ${state.e2eMs},")
        appendLine("  \"stages\": [")
        state.stages.forEachIndexed { index, stage ->
            val comma = if (index == state.stages.lastIndex) "" else ","
            appendLine("    {\"stage\": \"${stage.stage}\", \"meanMs\": ${stage.meanMs}, \"p99Ms\": ${stage.p99Ms}}$comma")
        }
        appendLine("  ]")
        appendLine("}")
    }

    companion object {
        /** E2E 折线保留的采样数。 */
        const val HISTORY_SIZE: Int = 120
    }
}

/** 便捷扩展：在给定 scope 上启动一个采集器。 */
fun PerfRecorder.launch(scope: CoroutineScope): Job = scope.launch { start() }
