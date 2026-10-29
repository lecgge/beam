package dev.beam.core.data.perf

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * 性能面板导出格式（§3.13 `Perf::export(fmt, path)`）。
 */
enum class PerfExportFormat { CSV, JSON }

/**
 * 单阶段耗时（均值 / P99，毫秒）。
 *
 * 阶段名与 §3.13 一致：抓屏 / 颜色转换 / 编码 / 分包 / RTT / 丢包 / FEC 恢复 /
 * 抖动缓冲 / 解码 / 渲染 / E2E。
 */
data class PerfStageTiming(
    val stage: String,
    val meanMs: Double = 0.0,
    val p99Ms: Double = 0.0,
)

/**
 * 性能面板的 UI 状态。
 *
 * 放在 `:core-data` 而不是 `:feature-perf`：`:feature-perf` 只在 debug 变体被
 * 依赖（§3.12 / §3.13 的 release 门禁），而 `:app` 的 debug / release 变体
 * 需要**同一个** `PerfEntry` 签名，因此共享模型必须放在两个变体都可见的模块里。
 */
data class PerfUiState(
    val fps: Double = 0.0,
    val bitrateKbps: Double = 0.0,
    val lossPercent: Double = 0.0,
    val fecRecoveredPercent: Double = 0.0,
    val droppedFrames: Long = 0L,
    val cpuPercent: Double = 0.0,
    val memoryMb: Double = 0.0,
    val rttMs: Double = 0.0,
    val e2eMs: Double = 0.0,
    val resolution: String = "",
    val codec: String = "",
    val stages: List<PerfStageTiming> = emptyList(),
    /** E2E 采样序列，用于画折线。 */
    val e2eHistory: List<Double> = emptyList(),
)

/**
 * 性能快照存储。
 *
 * `:feature-perf`（debug）写入，`:app` 的 debug 变体读取。
 * release 变体里没有任何写入方，面板始终展示空状态。
 */
object PerfSnapshotStore {

    private val _state = MutableStateFlow(PerfUiState())

    /** 当前快照。 */
    val state: StateFlow<PerfUiState> = _state.asStateFlow()

    /** 发布新快照。 */
    fun publish(next: PerfUiState) {
        _state.value = next
    }

    /** 清空（会话结束）。 */
    fun reset() {
        _state.value = PerfUiState()
    }
}
