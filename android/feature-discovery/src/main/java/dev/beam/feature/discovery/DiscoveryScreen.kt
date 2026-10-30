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
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusEvent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.beam.core.data.ui.BeamDialog
import dev.beam.core.data.ui.BeamTokens

/**
 * 主机列表页（原型「Android 端 → 主机列表」）。
 *
 * 只做展示与事件回调；所有状态与逻辑在 [DiscoveryViewModel] 里。
 *
 * ## 交互约定
 *
 * - **二次确认**（移除信任）走模态弹窗（[BeamDialog]）；
 * - **短反馈**（已复制地址、已移除信任）走 Toast（`BeamToast`，由 `:app` 挂宿主）；
 * - **错误**留在页面上（`NoticeCard`），不弹窗、不自动消失。
 */
@Composable
fun DiscoveryScreen(
    state: DiscoveryUiState,
    onHostClick: (HostRow) -> Unit,
    onPairClick: (HostRow) -> Unit,
    onRefresh: () -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
    onRemoveTrust: (HostRow) -> Unit = {},
    onManualConnect: (String) -> Unit = {},
    onCopyAddress: (HostRow) -> Unit = {},
) {
    // 待确认的「移除信任」目标。
    var pendingRemoval by remember { mutableStateOf<HostRow?>(null) }
    // 手动输入主机地址的弹窗。
    var manualVisible by remember { mutableStateOf(false) }
    var manualAddress by remember { mutableStateOf("") }

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
            EmptyState(
                onRefresh = onRefresh,
                onManual = { manualVisible = true },
            )
        } else {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(BeamTokens.SpacingMd)) {
                items(state.hosts, key = { it.peer.id + it.peer.address }) { row ->
                    HostCard(
                        row = row,
                        onClick = { onHostClick(row) },
                        onPair = { onPairClick(row) },
                        onCopyAddress = { onCopyAddress(row) },
                        onRequestRemoveTrust = { pendingRemoval = row },
                    )
                }
            }
        }
    }

    // 二次确认：移除信任。
    pendingRemoval?.let { row ->
        BeamDialog(
            visible = true,
            title = "移除信任",
            message = "移除「${row.peer.name}」后，下次连接需要重新配对（PIN 或扫码）。",
            confirmLabel = "移除",
            destructive = true,
            onConfirm = {
                pendingRemoval = null
                onRemoveTrust(row)
            },
            onDismiss = { pendingRemoval = null },
        )
    }

    // 手动输入主机地址（空态的次操作）。
    if (manualVisible) {
        BeamDialog(
            visible = true,
            title = "手动连接主机",
            message = "输入主机地址（ip 或 ip:port，默认端口 4827），随后用主机屏幕上显示的 PIN 完成配对。",
            confirmLabel = "下一步",
            onConfirm = {
                manualVisible = false
                val address = manualAddress
                manualAddress = ""
                if (address.isNotBlank()) onManualConnect(address)
            },
            onDismiss = {
                manualVisible = false
                manualAddress = ""
            },
        ) {
            OutlinedTextField(
                value = manualAddress,
                onValueChange = { manualAddress = it },
                singleLine = true,
                label = { Text("主机地址") },
                placeholder = { Text("192.168.1.10:4827") },
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
private fun HostCard(
    row: HostRow,
    onClick: () -> Unit,
    onPair: () -> Unit,
    onCopyAddress: () -> Unit,
    onRequestRemoveTrust: () -> Unit,
) {
    // TV（10 英尺）用 D-pad 导航：焦点可见、中心键激活。
    val focused = remember { mutableStateOf(false) }
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .onKeyEvent { event ->
                if (event.type == KeyEventType.KeyDown &&
                    (event.key == Key.DirectionCenter || event.key == Key.Enter)
                ) {
                    onClick()
                    true
                } else {
                    false
                }
            }
            .onFocusEvent { state -> focused.value = state.isFocused }
            .clickable(onClick = onClick),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(
            width = if (focused.value) 2.dp else 1.dp,
            color = if (focused.value) {
                MaterialTheme.colorScheme.primary
            } else {
                MaterialTheme.colorScheme.outline
            },
        ),
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
                Row(horizontalArrangement = Arrangement.spacedBy(BeamTokens.SpacingSm)) {
                    TextButton(onClick = onCopyAddress) { Text("复制地址") }
                    TextButton(onClick = onRequestRemoveTrust) { Text("移除信任") }
                }
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

/**
 * 空态（产品方案 §16「状态：空态(图标+说明+**主操作**)」）。
 *
 * 主操作是「重新扫描」；「手动输入 IP」作为次操作。
 */
@Composable
private fun EmptyState(
    onRefresh: () -> Unit,
    onManual: () -> Unit,
) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(vertical = 48.dp),
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
        Spacer(Modifier.height(BeamTokens.SpacingXl))
        Button(
            onClick = onRefresh,
            shape = RoundedCornerShape(BeamTokens.RadiusButton),
        ) {
            Text("重新扫描")
        }
        Spacer(Modifier.height(BeamTokens.SpacingSm))
        OutlinedButton(
            onClick = onManual,
            shape = RoundedCornerShape(BeamTokens.RadiusButton),
        ) {
            Text("手动输入 IP")
        }
    }
}
