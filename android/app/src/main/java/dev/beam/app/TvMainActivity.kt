package dev.beam.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge

/**
 * Android TV 入口（§3.12 `:app` 职责包含「TV 入口」）。
 *
 * 与手机端共用 [BeamRoot]，但传入 `isTv = true`：
 * - 字号整体放大（TV 观看距离更远，§3.12 `:app` 职责）；
 * - 主机列表项支持 D-pad 焦点导航（见 `:feature-discovery` 的 `HostCard`）。
 *
 * TV 端通过 `LEANBACK_LAUNCHER` 出现在系统启动器里，
 * 且 manifest 中 `android.software.leanback` 声明为 `required="false"`，
 * 因此同一个 APK 同时支持手机与 TV。
 */
class TvMainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val container = (application as BeamApplication).container
        setContent {
            BeamRoot(container = container, isTv = true)
        }
    }
}
