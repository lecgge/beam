package dev.beam.feature.discovery

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.beam.core.data.ui.BeamTokens

/**
 * 主机列表页（原型「Android 端 → 主机列表」）。
 *
 * 只做展示与事件回调；所有状态与逻辑在 [DiscoveryViewModel] 里。
 */
@Composable
fun DiscoveryScreen(
    state: DiscoveryUiState,
    onHostClick: (HostRow) -> Unit,
    onPairClick: (HostRow) -> Unit,
    onRefresh: () -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .padding(BeamTokens.SpacingXl),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = "Beam",
                style = MaterialTheme.typography.titleLarge,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onBackground,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = onRefresh) { Text("刷新") }
            TextButton(onClick = onOpenSettings) { Text("设置") }
        }

        Spacer(Modifier.height(BeamTokens.SpacingMd))

        if (!state.nativeReady) {
            NoticeCard(
                text = "原生库尚未构建：请按 core-net/README.md 生成 uniffi 绑定与 libbeam_ffi.so 后重新编译。",
                accent = BeamTokens.Warning,
            )
            Spacer(Modifier.height(BeamTokens.SpacingMd))
        }

        state.error?.let { message ->
            NoticeCard(text = message, accent = BeamTokens.Error)
            Spacer(Modifier.height(BeamTokens.SpacingMd))
        }

        Row(verticalAlignment = Alignment.CenterVertically) {
            if (state.isScanning) {
                CircularProgressIndicator(
                    modifier = Modifier.width(14.dp).height(14.dp),
                    strokeWidth = 2.dp,
                    color = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.width(BeamTokens.SpacingSm))
            }
            Text(
                text = if (state.isScanning) "正在搜索局域网…" else "搜索完成",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        Spacer(Modifier.height(BeamTokens.SpacingMd))

        if (state.isEmpty) {
            EmptyState()
            return@Column
        }

        LazyColumn(verticalArrangement = Arrangement.spacedBy(BeamTokens.SpacingMd)) {
            items(state.hosts, key = { it.peer.id + it.peer.address }) { row ->
                HostCard(
                    row = row,
                    onClick = { onHostClick(row) },
                    onPair = { onPairClick(row) },
                )
            }
        }
    }
}

@Composable
private fun HostCard(
    row: HostRow,
    onClick: () -> Unit,
    onPair: () -> Unit,
) {
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
    ) {
        Column(modifier = Modifier.padding(BeamTokens.SpacingLg)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = "🖥 ${row.peer.name}",
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                    color = MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier.weight(1f),
                )
                StatusPill(row)
            }
            Spacer(Modifier.height(BeamTokens.SpacingSm))
            Text(
                text = row.peer.address.ifBlank { row.peer.id },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (row.trusted) {
                Spacer(Modifier.height(BeamTokens.SpacingSm))
                Text(
                    text = "已配对 · 点击直接连接",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                )
            } else {
                Spacer(Modifier.height(BeamTokens.SpacingSm))
                TextButton(onClick = onPair) {
                    Text("配对")
                }
            }
        }
    }
}

@Composable
private fun StatusPill(row: HostRow) {
    val (label, color) = when {
        row.trusted -> "已配对" to BeamTokens.Success
        else -> "未配对" to MaterialTheme.colorScheme.onSurfaceVariant
    }
    Box(
        modifier = Modifier
            .border(1.dp, color, RoundedCornerShape(99.dp))
            .padding(horizontal = BeamTokens.SpacingMd, vertical = 2.dp),
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.labelSmall,
            color = color,
        )
    }
}

@Composable
private fun NoticeCard(text: String, accent: Color) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(1.dp, accent),
    ) {
        Text(
            text = text,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.padding(BeamTokens.SpacingLg),
        )
    }
}

@Composable
private fun EmptyState() {
    Column(
        modifier = Modifier.fillMaxWidth().padding(vertical = 64.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(text = "🔗", style = MaterialTheme.typography.headlineLarge)
        Spacer(Modifier.height(BeamTokens.SpacingMd))
        Text(
            text = "没有发现主机",
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.onSurface,
        )
        Spacer(Modifier.height(BeamTokens.SpacingSm))
        Text(
            text = "确认主机已开播，且与本机在同一局域网",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
