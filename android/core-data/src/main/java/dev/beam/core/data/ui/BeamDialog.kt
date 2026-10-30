package dev.beam.core.data.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

/**
 * Beam 模态弹窗（产品方案 §16「组件：…模态弹窗」）。
 *
 * 结构：**遮罩 + 面板 + 确认 / 取消**。
 *
 * ## 用法约定
 *
 * 只在**确实需要用户确认**的地方使用（删除已配对设备 / 移除信任 / 覆盖配置等）；
 * 普通提示仍然用页内 `NoticeCard`，**错误**也留在页面上，不用弹窗。
 *
 * ```kotlin
 * var visible by remember { mutableStateOf(false) }
 * BeamDialog(
 *     visible = visible,
 *     title = "移除信任",
 *     message = "移除后需要重新配对。",
 *     confirmLabel = "移除",
 *     destructive = true,
 *     onConfirm = { visible = false; doRemove() },
 *     onDismiss = { visible = false },
 * )
 * ```
 *
 * ## 为什么放在 `:core-data`
 *
 * 技术设计 §3.12 固定了 12 个模块，其中**没有** `:core-ui`；
 * 模态弹窗与 [BeamTokens] 一样属于「所有界面层模块的公共下层」，因此放在
 * `:core-data` 的 `ui` 包里（与 `BeamTokens` 的理由一致）。
 */
@Composable
fun BeamDialog(
    visible: Boolean,
    title: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    message: String? = null,
    confirmLabel: String = "确定",
    dismissLabel: String = "取消",
    destructive: Boolean = false,
    /** 面板内的自定义内容（例如输入框）。 */
    content: (@Composable () -> Unit)? = null,
) {
    if (!visible) return

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Box(
            modifier = modifier
                .fillMaxSize()
                // 遮罩：点按遮罩区域即触发 onDismiss（由 Dialog 处理）。
                .background(SCRIM),
            contentAlignment = Alignment.Center,
        ) {
            Surface(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(BeamTokens.SpacingXl),
                shape = RoundedCornerShape(BeamTokens.RadiusCard),
                color = MaterialTheme.colorScheme.surface,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
            ) {
                Column(modifier = Modifier.padding(BeamTokens.SpacingLg)) {
                    Text(
                        text = title,
                        style = MaterialTheme.typography.titleLarge,
                        fontWeight = FontWeight.SemiBold,
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                    if (!message.isNullOrBlank()) {
                        Spacer(Modifier.height(BeamTokens.SpacingMd))
                        Text(
                            text = message,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (content != null) {
                        Spacer(Modifier.height(BeamTokens.SpacingMd))
                        content()
                    }
                    Spacer(Modifier.height(BeamTokens.SpacingLg))
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.End,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        OutlinedButton(
                            onClick = onDismiss,
                            shape = RoundedCornerShape(BeamTokens.RadiusButton),
                        ) {
                            Text(dismissLabel)
                        }
                        Spacer(Modifier.width(BeamTokens.SpacingSm))
                        Button(
                            onClick = onConfirm,
                            shape = RoundedCornerShape(BeamTokens.RadiusButton),
                            colors = if (destructive) {
                                ButtonDefaults.buttonColors(
                                    containerColor = MaterialTheme.colorScheme.error,
                                    contentColor = MaterialTheme.colorScheme.onError,
                                )
                            } else {
                                ButtonDefaults.buttonColors()
                            },
                        ) {
                            Text(confirmLabel)
                        }
                    }
                }
            }
        }
    }
}

/** 遮罩颜色（半透明黑）。 */
private val SCRIM: Color = Color.Black.copy(alpha = 0.55f)
