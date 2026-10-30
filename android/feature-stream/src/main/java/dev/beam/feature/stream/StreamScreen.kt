package dev.beam.feature.stream

import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import dev.beam.core.data.ui.BeamTokens
import dev.beam.core.input.BeamGamepadInput
import dev.beam.core.input.beamKeyInput
import dev.beam.core.input.beamTouchInput
import dev.beam.core.net.InputEvent
import dev.beam.core.net.SessionStats

/**
 * 串流页（§3.12 `:feature-stream`）。
 *
 * 组成：
 * - `SurfaceView` 承载解码输出（§3.12「SurfaceView」）；
 * - 右上角**悬浮工具栏**（展开/收起）；
 * - 左上角**统计浮层**（消费 `session://stats`，500 ms 一次）；
 * - 触控 / 按键 / 遥控 / 手柄的**采集**（映射逻辑在 `:core-input`，本模块只负责接线）。
 *
 * ## 禁止事项
 * **不涉及解码细节**：解码器与节拍由 `:core-decoder` / `:core-renderer` 负责，
 * 本页只做 Surface 的交接与状态展示。
 */
@Composable
fun StreamScreen(
    state: StreamUiState,
    onSurfaceAvailable: (Surface) -> Unit,
    onSurfaceDestroyed: () -> Unit,
    onToggleOverlay: () -> Unit,
    onToggleToolbar: () -> Unit,
    onDisconnect: () -> Unit,
    onInput: (InputEvent) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current

    // 沉浸式：串流中隐藏状态栏与导航栏（粘性沉浸），离开时恢复（PP-04）。
    StreamImmersiveEffect(
        enabled = state.phase == StreamPhase.STREAMING || state.phase == StreamPhase.RECONNECTING,
    )

    // 手柄模拟轴走 Android 的 generic motion 通道（不在 Compose 的按键/指针通道里）。
    BeamGamepadInput(onEvent = onInput)

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(Color.Black)
            // 输入采集：Compose 触控 / 按键 → :core-input 映射 → 回传主机。
            .beamTouchInput(onEvent = onInput)
            .beamKeyInput(onEvent = onInput),
    ) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                SurfaceView(ctx).apply {
                    holder.addCallback(object : SurfaceHolder.Callback {
                        override fun surfaceCreated(holder: SurfaceHolder) {
                            onSurfaceAvailable(holder.surface)
                        }

                        override fun surfaceChanged(
                            holder: SurfaceHolder,
                            format: Int,
                            width: Int,
                            height: Int,
                        ) {
                            onSurfaceAvailable(holder.surface)
                        }

                        override fun surfaceDestroyed(holder: SurfaceHolder) {
                            onSurfaceDestroyed()
                        }
                    })
                }
            },
        )

        if (state.overlayVisible) {
            StatsOverlay(
                stats = state.stats,
                resolutionLabel = state.resolutionLabel,
                codecLabel = state.codecLabel,
                modifier = Modifier.align(Alignment.TopStart).padding(BeamTokens.SpacingMd),
            )
        }

        // 输入回传失败要**如实显示**（例如 Rust 侧尚未导出 send_input）。
        state.inputError?.let { reason ->
            Box(
                modifier = Modifier
                    .align(Alignment.BottomStart)
                    .padding(BeamTokens.SpacingMd)
                    .background(BeamTokens.OverlaySurface, RoundedCornerShape(8.dp))
                    .padding(horizontal = BeamTokens.SpacingMd, vertical = BeamTokens.SpacingSm),
            ) {
                Text(
                    text = "输入回传失败：$reason",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color(0xFFFFC107),
                )
            }
        }

        StreamToolbar(
            expanded = state.toolbarVisible,
            onToggle = onToggleToolbar,
            onToggleOverlay = onToggleOverlay,
            onDisconnect = onDisconnect,
            modifier = Modifier.align(Alignment.TopEnd).padding(BeamTokens.SpacingMd),
        )

        when (state.phase) {
            StreamPhase.CONNECTING -> CenterNotice("正在连接…")
            StreamPhase.RECONNECTING -> CenterNotice("连接中断，正在重连…（保留最后一帧）")
            StreamPhase.DISCONNECTED -> CenterNotice(state.message ?: "连接已断开")
            StreamPhase.ERROR -> CenterNotice(state.message ?: "串流出错")
            else -> Unit
        }
    }
}

@Composable
private fun CenterNotice(text: String) {
    Box(
        modifier = Modifier.fillMaxSize().background(Color(0xCC000000)),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            CircularProgressIndicator(
                modifier = Modifier.width(18.dp),
                strokeWidth = 2.dp,
                color = Color.White,
            )
            Spacer(Modifier.width(BeamTokens.SpacingSm))
            Text(text = text, color = Color.White, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/**
 * 统计浮层。
 *
 * 展示 `session://stats`（500 ms）的关键字段：端到端延迟、解码/渲染耗时、
 * 分辨率、编码、丢包与 FEC 恢复率。
 */
@Composable
private fun StatsOverlay(
    stats: SessionStats?,
    resolutionLabel: String,
    codecLabel: String,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .background(BeamTokens.OverlaySurface, RoundedCornerShape(10.dp))
            .border(1.dp, BeamTokens.OverlayOutline, RoundedCornerShape(10.dp))
            .padding(BeamTokens.SpacingMd),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val e2e = stats?.e2eMs ?: 0.0
            Text(
                text = "端到端 ",
                style = MaterialTheme.typography.bodySmall,
                color = Color(0xFFE8EDF6),
            )
            Text(
                text = String.format("%.1f ms", e2e),
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.SemiBold,
                color = if (e2e > 0 && e2e <= 30) BeamTokens.Success else BeamTokens.Warning,
            )
        }
        Text(
            text = "解码 ${fmt(stats?.decodeMs)} · 显示 ${fmt(stats?.renderMs)} ms",
            style = MaterialTheme.typography.bodySmall,
            color = Color(0xFF8F9AB0),
        )
        Text(
            text = "$resolutionLabel · $codecLabel",
            style = MaterialTheme.typography.bodySmall,
            color = Color(0xFF8F9AB0),
        )
        Text(
            text = "丢包 ${fmt(stats?.lossPercent)}% · FEC ${fmt(stats?.fecRecoveredPercent)}%",
            style = MaterialTheme.typography.bodySmall,
            color = Color(0xFF8F9AB0),
        )
    }
}

/**
 * 悬浮工具栏。
 *
 * 收起时只显示一个「工具」按钮；展开时提供统计浮层开关与断开按钮。
 */
@Composable
private fun StreamToolbar(
    expanded: Boolean,
    onToggle: () -> Unit,
    onToggleOverlay: () -> Unit,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(BeamTokens.SpacingXs),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (expanded) {
            ToolbarButton("📊", "统计浮层", onToggleOverlay)
            ToolbarButton("⏻", "断开", onDisconnect)
        }
        ToolbarButton(if (expanded) "›" else "‹ 工具", "工具栏", onToggle)
    }
}

@Composable
private fun ToolbarButton(text: String, description: String, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .background(BeamTokens.OverlaySurface, RoundedCornerShape(8.dp))
            .border(1.dp, BeamTokens.OverlayOutline, RoundedCornerShape(8.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = BeamTokens.SpacingMd, vertical = BeamTokens.SpacingSm),
    ) {
        Text(
            text = text,
            style = MaterialTheme.typography.bodySmall,
            color = Color.White,
        )
    }
}

private fun fmt(value: Double?): String = value?.let { String.format("%.1f", it) } ?: "—"
