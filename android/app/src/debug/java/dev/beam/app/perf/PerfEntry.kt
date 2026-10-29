package dev.beam.app.perf

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfSnapshotStore
import dev.beam.core.data.perf.PerfUiState
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
