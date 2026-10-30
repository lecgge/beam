package dev.beam.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import dev.beam.feature.stream.StreamImmersive

/**
 * 手机 / 平板入口。
 */
class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // 全屏沉浸（PP-04）：系统栏以「滑动临时显示」的方式出现，
        // 串流页由 :feature-stream 负责隐藏 / 恢复。
        StreamImmersive.configureEdgeToEdge(window)
        val container = (application as BeamApplication).container
        setContent {
            BeamRoot(container = container)
        }
    }
}
