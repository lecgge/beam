package dev.beam.app.perf

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfSnapshotStore
import dev.beam.core.data.perf.PerfUiState
import dev.beam.core.net.SessionStats
import dev.beam.core.net.SessionStatsTickSource
import dev.beam.feature.perf.PerfRecorder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.Flow
import java.io.File

/**
 * debug 变体的性能面板入口。
 *
 * 本文件只存在于 `src/debug` 源集；`src/release` 里有一个**同名同签名**的空实现，
 * 因此 `:app` 的 `main` 源集可以无条件调用，而 release 依赖图里**不含**
 * `:feature-perf`（§3.12 / §3.13 的 release 门禁）。
 */
object PerfEntry {

    /** release 构建为 false。 */
    const val available: Boolean = true

    /** 采集器实例（AN-23：性能面板必须有数据生产者）。 */
    private var recorder: PerfRecorder? = null

    /** 当前采集器对应的数据源（用于判断是不是同一次会话）。 */
    private var recordedStats: Flow<SessionStats>? = null

    /**
     * 开始采集。
     *
     * 数据源是**真实**的会话统计（`session://stats`，500 ms 一次）：
     * - `stats` 直接喂给 [PerfRecorder]，产出 KPI 与 E2E 折线；
     * - 同一份统计经 [SessionStatsTickSource] 拆成阶段耗时，产出阶段表。
     *
     * 没有会话就没有数据 —— 面板显示空态，**不填充任何合成数值**。
     *
     * 同一次会话（同一个数据源）重复调用时**不重建**采集器，
     * 以免重连时丢掉已经采到的 E2E 历史。
     */
    fun startRecording(scope: CoroutineScope, stats: Flow<SessionStats>) {
        if (recorder != null && recordedStats === stats) return
        stopRecording()
        val created = PerfRecorder(
            scope = scope,
            stats = stats,
            ticks = SessionStatsTickSource(stats).perfTicks(),
        )
        created.start()
        recorder = created
        recordedStats = stats
    }

    /** 停止采集。 */
    fun stopRecording() {
        recorder?.stop()
        recorder = null
        recordedStats = null
    }

    /** 渲染性能面板。 */
    @Composable
    fun Panel(
        state: PerfUiState,
        onExport: (PerfExportFormat) -> Unit,
        modifier: Modifier = Modifier,
    ) {
        dev.beam.feature.perf.PerfPanel(
            state = state,
            onExport = onExport,
            modifier = modifier,
        )
    }

    /**
     * 导出当前性能快照到应用缓存目录。
     *
     * @return 导出文件路径；失败时返回错误说明。
     */
    fun export(context: Context, format: PerfExportFormat): String = runCatching {
        val snapshot = PerfSnapshotStore.state.value
        val extension = when (format) {
            PerfExportFormat.CSV -> "csv"
            PerfExportFormat.JSON -> "json"
        }
        val content = when (format) {
            PerfExportFormat.CSV -> buildString {
                appendLine("stage,mean_ms,p99_ms")
                snapshot.stages.forEach { appendLine("${it.stage},${it.meanMs},${it.p99Ms}") }
            }
            PerfExportFormat.JSON -> buildString {
                appendLine("{")
                appendLine("  \"fps\": ${snapshot.fps},")
                appendLine("  \"bitrateKbps\": ${snapshot.bitrateKbps},")
                appendLine("  \"e2eMs\": ${snapshot.e2eMs},")
                appendLine("  \"stages\": [")
                snapshot.stages.forEachIndexed { index, stage ->
                    val comma = if (index == snapshot.stages.lastIndex) "" else ","
                    appendLine("    {\"stage\": \"${stage.stage}\", \"meanMs\": ${stage.meanMs}, \"p99Ms\": ${stage.p99Ms}}$comma")
                }
                appendLine("  ]")
                appendLine("}")
            }
        }
        val file = File(context.cacheDir, "beam-perf-${System.currentTimeMillis()}.$extension")
        file.writeText(content)
        file.absolutePath
    }.getOrElse { "导出失败：${it.message}" }
}
