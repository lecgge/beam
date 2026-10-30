package dev.beam.feature.stream

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.pm.ActivityInfo
import android.os.Build
import android.view.View
import android.view.Window
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalView

/**
 * 串流页的**沉浸式**（PP-04 / §3.12 `:app` 职责「全屏沉浸」）。
 *
 * ## 行为
 *
 * - **隐藏**状态栏与导航栏，画面铺满整块屏幕；
 * - **粘性沉浸**：用户从屏幕边缘滑动时系统栏临时显示，松手后自动隐藏；
 * - 离开串流页时**恢复**系统栏，避免影响其它页面。
 *
 * ## 实现选择
 *
 * 用平台自带的 [WindowInsetsController]（API 30+）与旧版
 * `systemUiVisibility` 标志（API 26–29）实现 —— 本工程 `minSdk = 26`，
 * 且不引入 `androidx.core` 的 `WindowInsetsControllerCompat`（保持依赖面最小）。
 */
object StreamImmersive {

    /**
     * Activity 级初始化：让系统栏以「滑动临时显示」的方式出现。
     *
     * 串流页隐藏系统栏后，用户滑动时系统栏应该**临时**出现再自动隐藏，
     * 而不是把系统栏常驻在画面上。
     */
    fun configureEdgeToEdge(window: Window) {
        window.addFlags(WindowManager.LayoutParams.FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.insetsController?.systemBarsBehavior =
                WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    /**
     * 切换沉浸式。
     *
     * @param immersive true = 隐藏系统栏（粘性沉浸）；false = 恢复显示。
     */
    fun apply(window: Window, immersive: Boolean) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val controller = window.insetsController ?: return
            controller.systemBarsBehavior =
                WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            if (immersive) {
                controller.hide(WindowInsets.Type.systemBars())
            } else {
                controller.show(WindowInsets.Type.systemBars())
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = if (immersive) {
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                    View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                    View.SYSTEM_UI_FLAG_FULLSCREEN or
                    View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                    View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                    View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
            } else {
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                    View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                    View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
            }
        }
    }
}

/**
 * 串流页的沉浸式副作用。
 *
 * 进入串流（[enabled] = true）时隐藏系统栏并把 Activity 切到横屏；
 * 离开时恢复系统栏与屏幕方向。
 *
 * 横屏是「串流时系统栏不干扰」的前提：竖屏下导航栏会一直占着画面底部。
 */
@Composable
fun StreamImmersiveEffect(enabled: Boolean = true) {
    val view = LocalView.current
    val activity = remember(view) { view.context.findActivity() }

    DisposableEffect(activity, enabled) {
        val window = activity?.window
        if (window == null || !enabled) {
            onDispose { }
        } else {
            StreamImmersive.apply(window, immersive = true)
            activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            onDispose {
                activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
                StreamImmersive.apply(window, immersive = false)
            }
        }
    }
}

/** 从任意 [Context] 找到宿主 Activity（Compose 的 context 可能是包装过的）。 */
internal fun Context.findActivity(): Activity? {
    var context = this
    while (context is ContextWrapper) {
        if (context is Activity) return context
        context = context.baseContext
    }
    return null
}
