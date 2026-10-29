package dev.beam.app

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.beam.app.theme.BeamTheme
import dev.beam.core.data.BeamSettings
import dev.beam.core.data.ThemeMode

/**
 * 应用根 Composable：主题 + 导航。
 *
 * 主题模式由用户在设置页选择（§3.12 `:feature-settings`），默认跟随系统。
 */
@Composable
fun BeamRoot(container: AppContainer) {
    val settings by container.settingsRepo.settings.collectAsStateWithLifecycle(
        initialValue = BeamSettings(),
    )
    val darkTheme = when (settings.themeMode) {
        ThemeMode.SYSTEM -> isSystemInDarkTheme()
        ThemeMode.LIGHT -> false
        ThemeMode.DARK -> true
    }
    BeamTheme(darkTheme = darkTheme) {
        RequestNotificationPermissionOnFirstLaunch()
        BeamNavHost(container = container)
    }
}
