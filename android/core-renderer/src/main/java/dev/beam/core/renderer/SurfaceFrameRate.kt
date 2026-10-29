package dev.beam.core.renderer

import android.os.Build
import android.view.Surface

/**
 * Surface 管理与刷新率设置（§3.12 `:core-renderer`）。
 *
 * Android 11（API 30）起 `Surface.setFrameRate` 可以把视频表面的刷新率提示给
 * 合成器 / 显示驱动，是 Android 端低延迟（对齐目标刷新率 vsync）的前提。
 */
object SurfaceFrameRate {

    /**
     * 设置表面的目标刷新率。
     *
     * @param surface 目标表面。
     * @param fps     目标帧率（如 60 / 120 / 165）。
     * @param compatibility 兼容性策略，API 30 起生效。
     *
     * API < 30 上是空操作：系统会按应用默认策略选刷新率。
     */
    fun apply(
        surface: Surface,
        fps: Int,
        compatibility: Int = Surface.FRAME_RATE_COMPATIBILITY_DEFAULT,
    ) {
        if (fps <= 0) return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        runCatching { surface.setFrameRate(fps.toFloat(), compatibility) }
    }

    /**
     * 切到「只要 60fps 也行」的兼容模式。
     *
     * 用于 Android 12+ 的 `FRAME_RATE_COMPATIBILITY_FIXED_SOURCE`：
     * 声明内容帧率固定，允许系统在其他刷新率下做兼容处理。
     */
    fun applyFixedSource(surface: Surface, fps: Int) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
        apply(surface, fps, Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE)
    }
}

/**
 * 表面来源。
 *
 * `:feature-stream` 用它把 `SurfaceView` 的 `Surface` 交给 `:core-renderer`
 * 与 `:core-decoder`，避免上层直接操作 `SurfaceHolder`。
 */
fun interface SurfaceSource {

    /** 当前可用的表面；尚未就绪时返回 null。 */
    fun surface(): Surface?
}
