package dev.beam.core.data

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.floatPreferencesKey
import androidx.datastore.preferences.core.intPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import dev.beam.core.net.VideoCodec
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.map
import java.io.IOException

/** DataStore 的键名集合。 */
private object SettingsKeys {
    val WIDTH = intPreferencesKey("stream_width")
    val HEIGHT = intPreferencesKey("stream_height")
    val FPS = intPreferencesKey("stream_fps")
    val CODEC = stringPreferencesKey("stream_codec")
    val BITRATE = intPreferencesKey("stream_bitrate_kbps")
    val FEC_RATIO = floatPreferencesKey("stream_fec_ratio")
    val AUDIO_ENABLED = booleanPreferencesKey("audio_enabled")
    val OVERLAY_ENABLED = booleanPreferencesKey("overlay_enabled")
    val DECODER_PREF = stringPreferencesKey("decoder_preference")
    val THEME_MODE = stringPreferencesKey("theme_mode")
}

/**
 * 设置仓库（§3.12 `:core-data` 的对外接口之一）。
 *
 * 基于 Jetpack DataStore Preferences 持久化；读取失败（文件损坏）时回退到
 * 默认值而不是崩溃。
 */
class SettingsRepo(
    private val dataStore: DataStore<Preferences>,
) {

    /** 当前设置（冷启动时先发默认值，再发持久化值）。 */
    val settings: Flow<BeamSettings> = dataStore.data
        .catch { cause ->
            if (cause is IOException) emit(emptyPreferences()) else throw cause
        }
        .map { it.toSettings() }

    /** 以函数式方式更新设置。 */
    suspend fun update(transform: (BeamSettings) -> BeamSettings) {
        dataStore.edit { prefs ->
            val next = transform(prefs.toSettings())
            prefs[SettingsKeys.WIDTH] = next.width
            prefs[SettingsKeys.HEIGHT] = next.height
            prefs[SettingsKeys.FPS] = next.fps
            prefs[SettingsKeys.CODEC] = next.codec.name
            prefs[SettingsKeys.BITRATE] = next.bitrateKbps
            prefs[SettingsKeys.FEC_RATIO] = next.fecRatio
            prefs[SettingsKeys.AUDIO_ENABLED] = next.audioEnabled
            prefs[SettingsKeys.OVERLAY_ENABLED] = next.overlayEnabled
            prefs[SettingsKeys.DECODER_PREF] = next.decoderPreference.name
            prefs[SettingsKeys.THEME_MODE] = next.themeMode.name
        }
    }

    /** 恢复内置默认值。 */
    suspend fun reset() = update { BeamSettings() }
}

private fun Preferences.toSettings(): BeamSettings {
    val defaults = BeamSettings()
    return BeamSettings(
        width = this[SettingsKeys.WIDTH] ?: defaults.width,
        height = this[SettingsKeys.HEIGHT] ?: defaults.height,
        fps = this[SettingsKeys.FPS] ?: defaults.fps,
        codec = this[SettingsKeys.CODEC]?.let { name ->
            runCatching { VideoCodec.valueOf(name) }.getOrDefault(defaults.codec)
        } ?: defaults.codec,
        bitrateKbps = this[SettingsKeys.BITRATE] ?: defaults.bitrateKbps,
        fecRatio = this[SettingsKeys.FEC_RATIO] ?: defaults.fecRatio,
        audioEnabled = this[SettingsKeys.AUDIO_ENABLED] ?: defaults.audioEnabled,
        overlayEnabled = this[SettingsKeys.OVERLAY_ENABLED] ?: defaults.overlayEnabled,
        decoderPreference = this[SettingsKeys.DECODER_PREF]?.let { name ->
            runCatching { DecoderPreference.valueOf(name) }.getOrDefault(defaults.decoderPreference)
        } ?: defaults.decoderPreference,
        themeMode = this[SettingsKeys.THEME_MODE]?.let { name ->
            runCatching { ThemeMode.valueOf(name) }.getOrDefault(defaults.themeMode)
        } ?: defaults.themeMode,
    )
}
