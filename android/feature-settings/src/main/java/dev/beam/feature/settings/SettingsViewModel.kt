package dev.beam.feature.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import dev.beam.core.data.BeamSettings
import dev.beam.core.data.DecoderPreference
import dev.beam.core.data.SettingsRepo
import dev.beam.core.data.ThemeMode
import dev.beam.core.net.VideoCodec
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * 设置页状态。
 */
data class SettingsUiState(
    val settings: BeamSettings = BeamSettings(),
    /** 是否已从 DataStore 读到持久化值。 */
    val loaded: Boolean = false,
)

/**
 * 设置 ViewModel。
 *
 * 状态用 [StateFlow] 暴露；写操作全部落到 [SettingsRepo]（DataStore），
 * 再由 Flow 回流刷新 UI，保证「单一数据源」。
 */
class SettingsViewModel(
    private val settingsRepo: SettingsRepo,
) : ViewModel() {

    private val _state = MutableStateFlow(SettingsUiState())
    val state: StateFlow<SettingsUiState> = _state.asStateFlow()

    init {
        viewModelScope.launch {
            settingsRepo.settings.collect { settings ->
                _state.update { it.copy(settings = settings, loaded = true) }
            }
        }
    }

    /** 设置分辨率。 */
    fun setResolution(width: Int, height: Int) = update { it.copy(width = width, height = height) }

    /** 设置帧率。 */
    fun setFps(fps: Int) = update { it.copy(fps = fps) }

    /** 设置编码格式。 */
    fun setCodec(codec: VideoCodec) = update { it.copy(codec = codec) }

    /** 设置码率（kbps）。 */
    fun setBitrate(kbps: Int) = update { it.copy(bitrateKbps = kbps) }

    /** 设置 FEC 冗余比例。 */
    fun setFecRatio(ratio: Float) = update { it.copy(fecRatio = ratio) }

    /** 开关音频。 */
    fun setAudioEnabled(enabled: Boolean) = update { it.copy(audioEnabled = enabled) }

    /** 开关统计浮层。 */
    fun setOverlayEnabled(enabled: Boolean) = update { it.copy(overlayEnabled = enabled) }

    /** 设置解码器偏好。 */
    fun setDecoderPreference(preference: DecoderPreference) = update { it.copy(decoderPreference = preference) }

    /** 设置主题模式。 */
    fun setThemeMode(mode: ThemeMode) = update { it.copy(themeMode = mode) }

    /**
     * 恢复默认。
     *
     * @return 是否写入成功；调用方据此决定提示「已恢复」还是「恢复失败」。
     */
    suspend fun reset(): Boolean = runCatching { settingsRepo.reset() }.isSuccess

    private fun update(transform: (BeamSettings) -> BeamSettings) {
        viewModelScope.launch {
            settingsRepo.update(transform)
        }
    }

    companion object {

        /** 手动构造函数注入用的工厂（不引入 DI 框架）。 */
        fun factory(settingsRepo: SettingsRepo): ViewModelProvider.Factory =
            object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T =
                    SettingsViewModel(settingsRepo) as T
            }
    }
}
