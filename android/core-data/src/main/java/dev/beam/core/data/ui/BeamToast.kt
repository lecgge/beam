package dev.beam.core.data.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * Toast 控制器（产品方案 §16「组件：…Toast」）。
 *
 * 语义：**短提示，自动消失**，不需要用户确认。
 *
 * - 只用于「已复制」「已保存」这类**成功反馈**；
 * - **错误不走 Toast**：错误必须留在页面上（`NoticeCard`），否则会被自动消失吞掉。
 *
 * @param scope 计时用的作用域（[show] 之后 [durationMs] 毫秒自动消失）。
 * @param durationMs 自动消失的延迟。
 */
class BeamToastController(
    private val scope: CoroutineScope,
    private val durationMs: Long = DEFAULT_DURATION_MS,
) {

    private val _message = MutableStateFlow<String?>(null)

    /** 当前提示文案；null 表示没有提示。 */
    val message: StateFlow<String?> = _message.asStateFlow()

    private var dismissJob: Job? = null

    /** 显示一条短提示（重复调用会覆盖上一条并重新计时）。 */
    fun show(text: String) {
        dismissJob?.cancel()
        _message.value = text
        dismissJob = scope.launch {
            delay(durationMs)
            _message.value = null
        }
    }

    /** 立即收起当前提示。 */
    fun dismiss() {
        dismissJob?.cancel()
        dismissJob = null
        _message.value = null
    }

    companion object {
        /** 默认显示时长：2 秒。 */
        const val DEFAULT_DURATION_MS: Long = 2_000L
    }
}

/**
 * 进程级 Toast 控制器。
 *
 * 界面层（`:feature-*`）直接 `BeamToast.show("已复制")`，
 * 由 `:app` 在根节点挂一个 [BeamToastHost] 负责渲染。
 */
object BeamToast {

    private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val controller: BeamToastController = BeamToastController(scope)

    /** 当前提示文案。 */
    val message: StateFlow<String?> get() = controller.message

    /** 显示一条短提示。 */
    fun show(text: String) = controller.show(text)

    /** 收起当前提示。 */
    fun dismiss() = controller.dismiss()
}

/**
 * Toast 宿主：把 [BeamToast] 的当前提示渲染在界面底部。
 *
 * 挂在应用根节点（见 `:app` 的 `BeamRoot`），一个应用只需要一个。
 */
@Composable
fun BeamToastHost(modifier: Modifier = Modifier) {
    val message by BeamToast.message.collectAsState()
    val text = message ?: return

    Box(modifier = modifier, contentAlignment = Alignment.BottomCenter) {
        Text(
            text = text,
            style = MaterialTheme.typography.bodyMedium,
            color = Color.White,
            modifier = Modifier
                .padding(horizontal = BeamTokens.SpacingXl, vertical = BeamTokens.SpacingSm)
                .background(TOAST_SURFACE, RoundedCornerShape(BeamTokens.RadiusButton))
                .padding(horizontal = BeamTokens.SpacingLg, vertical = BeamTokens.SpacingSm),
        )
    }
}

/** Toast 底色（深色浮层，§16「串流浮层」同族）。 */
private val TOAST_SURFACE: Color = Color(0xE6121826)
