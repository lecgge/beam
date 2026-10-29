package dev.beam.core.data.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Beam 设计令牌（产品方案 §16）。
 *
 * ## 为什么令牌在 `:core-data`
 *
 * 技术设计文档 §2 固定了 Android 端的 12 个模块，其中**没有** `:core-ui`，
 * 而设计令牌需要被 `:feature-*` 与 `:app` 同时引用。`:core-data` 是所有界面层
 * 模块的公共下��，因此把**纯数据**的设计令牌放在这里；
 * Material3 的 `ColorScheme` / `Typography` 组装则属于 `:app`（§3.12 `:app` 职责
 * 包含「主题」），见 `app/src/main/java/dev/beam/app/theme/BeamTheme.kt`。
 *
 * ## 取值来源
 * 产品方案 §16「设计规范」表；原型 `docs/prototype/interactive.html` 的 CSS 变量。
 */
object BeamTokens {

    // ------------------------------------------------------------ 颜色（深色）

    /** 页面底色。 */
    val DarkBackground = Color(0xFF0B0F17)

    /** 卡片 / 面板底色。 */
    val DarkPanel = Color(0xFF121826)

    /** 次级面板底色。 */
    val DarkPanelAlt = Color(0xFF0E1420)

    /** 分隔线 / 描边。 */
    val DarkLine = Color(0xFF222B3D)

    /** 正文文字。 */
    val DarkText = Color(0xFFE8EDF6)

    /** 次要文字。 */
    val DarkTextSub = Color(0xFF8F9AB0)

    /** 主色（深色）。 */
    val DarkPrimary = Color(0xFF5B7CFF)

    /** 状态胶囊底色（深色）。 */
    val DarkChip = Color(0xFF1A2340)

    // ------------------------------------------------------------ 颜色（浅色）

    /** 页面底色。 */
    val LightBackground = Color(0xFFF4F6FB)

    /** 卡片 / 面板底色。 */
    val LightPanel = Color(0xFFFFFFFF)

    /** 次级面板底色。 */
    val LightPanelAlt = Color(0xFFEEF1F8)

    /** 分隔线 / 描边。 */
    val LightLine = Color(0xFFDFE4EE)

    /** 正文文字。 */
    val LightText = Color(0xFF151B28)

    /** 次要文字。 */
    val LightTextSub = Color(0xFF5D6880)

    /** 主色（浅色）。 */
    val LightPrimary = Color(0xFF3D5CF5)

    /** 状态胶囊底色（浅色）。 */
    val LightChip = Color(0xFFE7ECFF)

    // ------------------------------------------------------------ 状态色

    /** 成功。 */
    val Success = Color(0xFF22C55E)

    /** 警告。 */
    val Warning = Color(0xFFF59E0B)

    /** 错误。 */
    val Error = Color(0xFFEF4444)

    /** 串流浮层底色（`#0a0e18` 85%）。 */
    val OverlaySurface = Color(0xD90A0E18)

    /** 串流浮层描边。 */
    val OverlayOutline = Color(0xFF2A3348)

    // ------------------------------------------------------------ 间距（4 的倍数）

    /** 4dp。 */
    val SpacingXs = 4.dp

    /** 8dp。 */
    val SpacingSm = 8.dp

    /** 12dp。 */
    val SpacingMd = 12.dp

    /** 14dp（卡片内边距）。 */
    val SpacingLg = 14.dp

    /** 20dp（页边距）。 */
    val SpacingXl = 20.dp

    // ------------------------------------------------------------ 圆角

    /** 按钮圆角 8dp。 */
    val RadiusButton = 8.dp

    /** 卡片圆角 12dp。 */
    val RadiusCard = 12.dp

    /** 手机外框圆角 30dp（原型用）。 */
    val RadiusPhone = 30.dp

    // ------------------------------------------------------------ 字号

    /** 标题 18 / 600。 */
    val TitleSize = 18.sp

    /** 正文 14 / 400。 */
    val BodySize = 14.sp

    /** 辅助文字 12 / 400。 */
    val CaptionSize = 12.sp

    /** PIN 输入 26 / 700。 */
    val PinSize = 26.sp
}
