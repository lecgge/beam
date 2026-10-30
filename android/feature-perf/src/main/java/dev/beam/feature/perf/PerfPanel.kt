package dev.beam.feature.perf

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfUiState
import dev.beam.core.data.ui.BeamTokens

/**
 * 性能面板（§3.12 `:feature-perf` 的对外接口 / §3.13）。
 *
 * **只存在于 debug 构建**：`:app` 通过 `debugImplementation(project(":feature-perf"))`
 * 引入，release 依赖图里没有本模块（§3.13 门控）。
 *
 * 布局对齐原型「性能统计」页：KPI 卡片网格 + E2E 折线 + 阶段耗时表 + 导出按钮。
 */
@Composable
fun PerfPanel(
    state: PerfUiState,
    onExport: (PerfExportFormat) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState())
            .padding(BeamTokens.SpacingXl),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = "性能统计",
                style = MaterialTheme.typography.titleLarge,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onBackground,
                modifier = Modifier.weight(1f),
            )
            StatusPill("仅 Debug 构建")
        }
        Spacer(Modifier.height(BeamTokens.SpacingMd))

        // 没有采样就如实说明空态，不用 0 值冒充「实测为 0」。
        if (state.stages.isEmpty() && state.e2eHistory.isEmpty()) {
            Surface(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(BeamTokens.RadiusCard),
                color = MaterialTheme.colorScheme.surface,
                border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
            ) {
                Text(
                    text = "暂无采样数据：debug 构建下开始串流后自动采集（§3.13）",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(BeamTokens.SpacingMd),
                )
            }
            Spacer(Modifier.height(BeamTokens.SpacingMd))
        }

        KpiGrid(state)

        Spacer(Modifier.height(BeamTokens.SpacingMd))
        E2eChart(state)

        Spacer(Modifier.height(BeamTokens.SpacingMd))
        StageTable(state)

        Spacer(Modifier.height(BeamTokens.SpacingMd))
        Row(horizontalArrangement = Arrangement.spacedBy(BeamTokens.SpacingSm)) {
            Button(onClick = { onExport(PerfExportFormat.CSV) }) { Text("导出 CSV") }
            OutlinedButton(onClick = { onExport(PerfExportFormat.JSON) }) { Text("导出 JSON") }
        }
    }
}

@Composable
private fun KpiGrid(state: PerfUiState) {
    val items = listOf(
        "帧率" to String.format("%.0f", state.fps),
        "码率" to String.format("%.1f Mbps", state.bitrateKbps / 1000.0),
        "丢包" to String.format("%.2f%%", state.lossPercent),
        "FEC 恢复" to String.format("%.0f%%", state.fecRecoveredPercent),
        "掉帧" to state.droppedFrames.toString(),
        "RTT" to String.format("%.1f ms", state.rttMs),
        "端到端" to String.format("%.1f ms", state.e2eMs),
        "分辨率" to state.resolution,
    )
    Column(verticalArrangement = Arrangement.spacedBy(BeamTokens.SpacingSm)) {
        items.chunked(2).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(BeamTokens.SpacingSm)) {
                row.forEach { (label, value) ->
                    Surface(
                        modifier = Modifier.weight(1f),
                        shape = RoundedCornerShape(BeamTokens.RadiusCard),
                        color = MaterialTheme.colorScheme.surface,
                        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
                    ) {
                        Column(modifier = Modifier.padding(BeamTokens.SpacingMd)) {
                            Text(
                                text = label,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Text(
                                text = value,
                                style = MaterialTheme.typography.titleMedium,
                                color = MaterialTheme.colorScheme.onSurface,
                            )
                        }
                    }
                }
            }
        }
    }
}

/**
 * 端到端延迟折线。
 *
 * 虚线是 16.7 ms（60 fps 一帧）参考线。
 */
@Composable
private fun E2eChart(state: PerfUiState) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
    ) {
        Column(modifier = Modifier.padding(BeamTokens.SpacingMd)) {
            Text(
                text = "端到端延迟 ms（虚线 16.7）",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(BeamTokens.SpacingSm))
            Canvas(modifier = Modifier.fillMaxWidth().height(60.dp)) {
                val reference = 16.7
                val maxValue = maxOf(reference, state.e2eHistory.maxOrNull() ?: reference) * 1.2
                val refY = size.height * (1f - (reference / maxValue).toFloat())
                drawLine(
                    color = Color.Gray,
                    start = Offset(0f, refY),
                    end = Offset(size.width, refY),
                    strokeWidth = 1f,
                )
                if (state.e2eHistory.size >= 2) {
                    val step = size.width / (state.e2eHistory.size - 1)
                    val points = state.e2eHistory.mapIndexed { index, value ->
                        Offset(
                            x = index * step,
                            y = size.height * (1f - (value / maxValue).toFloat()),
                        )
                    }
                    for (i in 0 until points.size - 1) {
                        drawLine(
                            color = BeamTokens.DarkPrimary,
                            start = points[i],
                            end = points[i + 1],
                            strokeWidth = 2f,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun StageTable(state: PerfUiState) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
    ) {
        Column(modifier = Modifier.padding(BeamTokens.SpacingMd)) {
            if (state.stages.isEmpty()) {
                Text(
                    text = "暂无阶段采样（需要 debug 构建且串流进行中）",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                state.stages.forEach { stage ->
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = BeamTokens.SpacingXs),
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Text(
                            text = stage.stage,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurface,
                        )
                        Text(
                            text = "均值 ${String.format("%.2f", stage.meanMs)} · P99 ${String.format("%.2f", stage.p99Ms)} ms",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun StatusPill(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.labelSmall,
        color = BeamTokens.Warning,
        modifier = Modifier
            .background(BeamTokens.OverlaySurface, RoundedCornerShape(99.dp))
            .border(1.dp, BeamTokens.Warning, RoundedCornerShape(99.dp))
            .padding(horizontal = BeamTokens.SpacingMd, vertical = 2.dp),
    )
}
