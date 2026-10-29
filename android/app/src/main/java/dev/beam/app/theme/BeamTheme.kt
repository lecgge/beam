package dev.beam.app.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import dev.beam.core.data.ui.BeamTokens

/**
 * Beam 主题（§3.12 `:app` 职责：导航、**主题**、权限、TV 入口）。
 *
 * 取值来自产品方案 §16「设计规范」；深浅两套自动跟随系统。
 *
 * ## 字体
 * 使用 `FontFamily.SansSerif`：Android 系统字体族里的 CJK 字形默认就是
 * **Noto Sans CJK**，与产品方案 §16「Android Noto Sans CJK」一致。
 */
private val DarkColors = darkColorScheme(
    primary = BeamTokens.DarkPrimary,
    onPrimary = Color.White,
    primaryContainer = BeamTokens.DarkChip,
    onPrimaryContainer = BeamTokens.DarkPrimary,
    secondary = BeamTokens.DarkPrimary,
    onSecondary = Color.White,
    secondaryContainer = BeamTokens.DarkChip,
    onSecondaryContainer = BeamTokens.DarkText,
    background = BeamTokens.DarkBackground,
    onBackground = BeamTokens.DarkText,
    surface = BeamTokens.DarkPanel,
    onSurface = BeamTokens.DarkText,
    surfaceVariant = BeamTokens.DarkPanelAlt,
    onSurfaceVariant = BeamTokens.DarkTextSub,
    surfaceTint = BeamTokens.DarkPrimary,
    outline = BeamTokens.DarkLine,
    outlineVariant = BeamTokens.DarkLine,
    error = BeamTokens.Error,
    onError = Color.White,
    errorContainer = BeamTokens.Error,
    onErrorContainer = Color.White,
    scrim = Color(0xCC000000),
)

private val LightColors = lightColorScheme(
    primary = BeamTokens.LightPrimary,
    onPrimary = Color.White,
    primaryContainer = BeamTokens.LightChip,
    onPrimaryContainer = BeamTokens.LightPrimary,
    secondary = BeamTokens.LightPrimary,
    onSecondary = Color.White,
    secondaryContainer = BeamTokens.LightChip,
    onSecondaryContainer = BeamTokens.LightText,
    background = BeamTokens.LightBackground,
    onBackground = BeamTokens.LightText,
    surface = BeamTokens.LightPanel,
    onSurface = BeamTokens.LightText,
    surfaceVariant = BeamTokens.LightPanelAlt,
    onSurfaceVariant = BeamTokens.LightTextSub,
    surfaceTint = BeamTokens.LightPrimary,
    outline = BeamTokens.LightLine,
    outlineVariant = BeamTokens.LightLine,
    error = BeamTokens.Error,
    onError = Color.White,
    errorContainer = Color(0xFFFDE7E7),
    onErrorContainer = BeamTokens.Error,
    scrim = Color(0x99000000),
)

/**
 * 字体规格（产品方案 §16：标题 18/600、正文 14/400、辅助 12/400、PIN 26/700）。
 *
 * @param scale 字号缩放系数；TV（10 英尺观看距离）用更大的字号。
 */
private fun beamTypography(scale: Float): Typography = Typography(
    titleLarge = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = (BeamTokens.TitleSize.value * scale).sp,
        lineHeight = (24f * scale).sp,
    ),
    titleMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = (16f * scale).sp,
        lineHeight = (22f * scale).sp,
    ),
    bodyLarge = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = (BeamTokens.BodySize.value * scale).sp,
        lineHeight = (20f * scale).sp,
    ),
    bodyMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = (BeamTokens.BodySize.value * scale).sp,
        lineHeight = (20f * scale).sp,
    ),
    bodySmall = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = (BeamTokens.CaptionSize.value * scale).sp,
        lineHeight = (16f * scale).sp,
    ),
    labelLarge = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Medium,
        fontSize = (BeamTokens.BodySize.value * scale).sp,
        lineHeight = (20f * scale).sp,
    ),
    labelSmall = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = (BeamTokens.CaptionSize.value * scale).sp,
        lineHeight = (16f * scale).sp,
    ),
)

/**
 * 应用主题。
 *
 * @param darkTheme 是否深色；默认跟随系统。
 * @param tv 是否 TV 形态：TV 观看距离更远，字号整体放大（§3.12 `:app` 职责含「TV 入口」）。
 */
@Composable
fun BeamTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    tv: Boolean = false,
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (darkTheme) DarkColors else LightColors,
        typography = beamTypography(if (tv) TV_TYPE_SCALE else 1f),
        content = content,
    )
}

/** TV 形态的字号缩放系数（10 英尺观看距离）。 */
private const val TV_TYPE_SCALE: Float = 1.4f
