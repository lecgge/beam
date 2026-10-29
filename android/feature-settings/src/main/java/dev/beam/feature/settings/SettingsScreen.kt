package dev.beam.feature.settings

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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.beam.core.data.BeamSettings
import dev.beam.core.data.DecoderPreference
import dev.beam.core.data.ThemeMode
import dev.beam.core.data.ui.BeamTokens
import dev.beam.core.net.VideoCodec

/**
 * 串流设置页（产品方案 §7 `:feature-settings`）。
 *
 * 包含：画质（分辨率/帧率）、编码、解码器、统计浮层开关（原型 Android 端设置页）。
 */
@Composable
fun SettingsScreen(
    state: SettingsUiState,
    onBack: () -> Unit,
    onResolutionChange: (Int, Int) -> Unit,
    onFpsChange: (Int) -> Unit,
    onCodecChange: (VideoCodec) -> Unit,
    onBitrateChange: (Int) -> Unit,
    onFecChange: (Float) -> Unit,
    onAudioChange: (Boolean) -> Unit,
    onOverlayChange: (Boolean) -> Unit,
    onDecoderPreferenceChange: (DecoderPreference) -> Unit,
    onThemeChange: (ThemeMode) -> Unit,
    onReset: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val settings = state.settings
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .verticalScroll(rememberScrollState())
            .padding(BeamTokens.SpacingXl),
    ) {
        TextButton(onClick = onBack) { Text("← 返回") }
        Text(
            text = "串流设置",
            style = MaterialTheme.typography.titleLarge,
            fontWeight = FontWeight.SemiBold,
            color = MaterialTheme.colorScheme.onBackground,
        )
        Spacer(Modifier.height(BeamTokens.SpacingMd))

        SettingsCard(title = "画质") {
            SegmentedRow(
                label = "分辨率",
                options = RESOLUTIONS.map { "${it.first}x${it.second}" },
                selected = "${settings.width}x${settings.height}",
                onSelect = { index ->
                    val (w, h) = RESOLUTIONS[index]
                    onResolutionChange(w, h)
                },
            )
            SegmentedRow(
                label = "帧率",
                options = FPS_OPTIONS.map { it.toString() },
                selected = settings.fps.toString(),
                onSelect = { index -> onFpsChange(FPS_OPTIONS[index]) },
            )
            SegmentedRow(
                label = "码率",
                options = BITRATE_OPTIONS.map { "$it Mbps" },
                selected = "${settings.bitrateKbps / 1000} Mbps",
                onSelect = { index -> onBitrateChange(BITRATE_OPTIONS[index] * 1000) },
            )
        }

        Spacer(Modifier.height(BeamTokens.SpacingMd))

        SettingsCard(title = "编码") {
            SegmentedRow(
                label = "格式",
                options = listOf("H.264", "HEVC", "AV1"),
                selected = when (settings.codec) {
                    VideoCodec.H264 -> "H.264"
                    VideoCodec.HEVC -> "HEVC"
                    VideoCodec.AV1 -> "AV1"
                },
                onSelect = { index -> onCodecChange(VideoCodec.entries[index]) },
            )
            SegmentedRow(
                label = "FEC 冗余",
                options = FEC_OPTIONS.map { "${(it * 100).toInt()}%" },
                selected = "${(settings.fecRatio * 100).toInt()}%",
                onSelect = { index -> onFecChange(FEC_OPTIONS[index]) },
            )
        }

        Spacer(Modifier.height(BeamTokens.SpacingMd))

        SettingsCard(title = "解码器") {
            SegmentedRow(
                label = "偏好",
                options = listOf("自动", "低延迟", "系统"),
                selected = when (settings.decoderPreference) {
                    DecoderPreference.AUTO -> "自动"
                    DecoderPreference.LOW_LATENCY -> "低延迟"
                    DecoderPreference.SYSTEM -> "系统"
                },
                onSelect = { index -> onDecoderPreferenceChange(DecoderPreference.entries[index]) },
            )
            Text(
                text = decoderHint(settings.decoderPreference),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        Spacer(Modifier.height(BeamTokens.SpacingMd))

        SettingsCard(title = "显示与音频") {
            SwitchRow(label = "统计浮层", checked = settings.overlayEnabled, onChange = onOverlayChange)
            SwitchRow(label = "音频", checked = settings.audioEnabled, onChange = onAudioChange)
            SegmentedRow(
                label = "主题",
                options = listOf("跟随系统", "浅色", "深色"),
                selected = when (settings.themeMode) {
                    ThemeMode.SYSTEM -> "跟随系统"
                    ThemeMode.LIGHT -> "浅色"
                    ThemeMode.DARK -> "深色"
                },
                onSelect = { index -> onThemeChange(ThemeMode.entries[index]) },
            )
        }

        Spacer(Modifier.height(BeamTokens.SpacingMd))

        TextButton(onClick = onReset) { Text("恢复默认设置") }
        Spacer(Modifier.height(BeamTokens.SpacingXl))
    }
}

@Composable
private fun SettingsCard(title: String, content: @Composable () -> Unit) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
    ) {
        Column(modifier = Modifier.padding(BeamTokens.SpacingLg)) {
            Text(
                text = title,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onSurface,
            )
            Spacer(Modifier.height(BeamTokens.SpacingMd))
            content()
        }
    }
}

@Composable
private fun SegmentedRow(
    label: String,
    options: List<String>,
    selected: String,
    onSelect: (Int) -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth().padding(bottom = BeamTokens.SpacingSm)) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(BeamTokens.SpacingXs))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(BeamTokens.SpacingXs),
        ) {
            options.forEachIndexed { index, option ->
                val isSelected = option == selected
                Box(
                    modifier = Modifier
                        .border(
                            width = 1.dp,
                            color = if (isSelected) MaterialTheme.colorScheme.primary
                            else MaterialTheme.colorScheme.outline,
                            shape = RoundedCornerShape(BeamTokens.RadiusButton),
                        )
                        .background(
                            if (isSelected) MaterialTheme.colorScheme.primary
                            else MaterialTheme.colorScheme.surface,
                            RoundedCornerShape(BeamTokens.RadiusButton),
                        )
                        .clickable { onSelect(index) }
                        .padding(horizontal = BeamTokens.SpacingMd, vertical = BeamTokens.SpacingSm),
                ) {
                    Text(
                        text = option,
                        style = MaterialTheme.typography.labelLarge,
                        color = if (isSelected) MaterialTheme.colorScheme.onPrimary
                        else MaterialTheme.colorScheme.onSurface,
                    )
                }
            }
        }
    }
}

@Composable
private fun SwitchRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = BeamTokens.SpacingXs),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
        )
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

private fun decoderHint(preference: DecoderPreference): String = when (preference) {
    DecoderPreference.AUTO -> "自动探测设备上的硬件解码器，并优先厂商 low-latency 型号。"
    DecoderPreference.LOW_LATENCY -> "强制优先厂商 low-latency 解码器；部分机型可能没有对应型号。"
    DecoderPreference.SYSTEM -> "使用系统默认解码器，兼容性最好但延迟可能更高。"
}

/** 可选分辨率。 */
private val RESOLUTIONS = listOf(1280 to 720, 1920 to 1080, 2560 to 1440, 3840 to 2160)

/** 可选帧率。 */
private val FPS_OPTIONS = listOf(30, 60, 120)

/** 可选码率（Mbps）。 */
private val BITRATE_OPTIONS = listOf(10, 20, 40, 80)

/** 可选 FEC 冗余比例。 */
private val FEC_OPTIONS = listOf(0.1f, 0.2f, 0.3f)
