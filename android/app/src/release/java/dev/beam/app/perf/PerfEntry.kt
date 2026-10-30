package dev.beam.app.perf

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfUiState
import dev.beam.core.net.SessionStats
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.Flow

/**
 * release 变体的性能面板入口：**空实现**。
 *
 * §3.13 要求 release 构建里不得出现任何 perf 类；`:app` 用
 * `debugImplementation(project(":feature-perf"))` 引入真正的实现，
 * 因此 release 变体的依赖图里没有 `:feature-perf`，这里也不引用它。
 */
object PerfEntry {

    /** release 构建为 false。 */
    const val available: Boolean = false

    /** release 不采集性能数据。 */
    fun startRecording(scope: CoroutineScope, stats: Flow<SessionStats>) = Unit

    /** release 不采集性能数据。 */
    fun stopRecording() = Unit

    /** 不渲染任何内容。 */
    @Composable
    fun Panel(
        state: PerfUiState,
        onExport: (PerfExportFormat) -> Unit,
        modifier: Modifier = Modifier,
    ) {
        // release 不提供性能面板
    }

    /** release 不导出性能数据。 */
    fun export(context: Context, format: PerfExportFormat): String = ""
}
