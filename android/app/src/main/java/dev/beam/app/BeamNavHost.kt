package dev.beam.app

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfSnapshotStore
import dev.beam.feature.discovery.DiscoveryScreen
import dev.beam.feature.discovery.DiscoveryViewModel
import dev.beam.feature.discovery.NsdHostScanner
import dev.beam.feature.pairing.PairingScreen
import dev.beam.feature.pairing.PairingViewModel
import dev.beam.feature.settings.SettingsScreen
import dev.beam.feature.settings.SettingsViewModel
import dev.beam.feature.stream.StreamScreen
import dev.beam.feature.stream.StreamViewModel

/** 路由表。 */
object Routes {
    /** 主机列表。 */
    const val HOSTS: String = "hosts"

    /** 配对。 */
    const val PAIRING: String = "pairing"

    /** 串流。 */
    const val STREAM: String = "stream"

    /** 设置。 */
    const val SETTINGS: String = "settings"

    /** 性能面板（仅 debug 构建可达）。 */
    const val PERF: String = "perf"
}

/**
 * 导航图。
 *
 * 5 个页面与原型 `docs/prototype/interactive.html` 的 Android 端一一对应：
 * 主机列表 / 配对 / 串流 / 设置 / 性能(debug)。
 */
@Composable
fun BeamNavHost(
    container: AppContainer,
    navController: NavHostController = rememberNavController(),
) {
    val context = LocalContext.current
    val selectedPeer by container.selectedPeer.collectAsStateWithLifecycle()

    NavHost(navController = navController, startDestination = Routes.HOSTS) {

        composable(Routes.HOSTS) {
            val viewModel: DiscoveryViewModel = viewModel(
                factory = DiscoveryViewModel.factory(
                    client = container.client,
                    hostRepo = container.hostRepo,
                    fallbackScanner = remember { NsdHostScanner(context) },
                    nativeReady = container.nativeReady,
                ),
            )
            val state by viewModel.state.collectAsStateWithLifecycle()
            LaunchedEffect(Unit) { viewModel.start() }

            DiscoveryScreen(
                state = state,
                onHostClick = { row ->
                    container.selectedPeer.value = row.peer
                    navController.navigate(if (row.trusted) Routes.STREAM else Routes.PAIRING)
                },
                onPairClick = { row ->
                    container.selectedPeer.value = row.peer
                    navController.navigate(Routes.PAIRING)
                },
                onRefresh = { viewModel.refresh() },
                onOpenSettings = { navController.navigate(Routes.SETTINGS) },
            )
        }

        composable(Routes.PAIRING) {
            val viewModel: PairingViewModel = viewModel(
                factory = PairingViewModel.factory(
                    client = container.client,
                    hostRepo = container.hostRepo,
                ),
            )
            val state by viewModel.state.collectAsStateWithLifecycle()
            val requestCamera = rememberPermissionRequest(BeamPermissions.CAMERA)

            LaunchedEffect(selectedPeer?.id) {
                selectedPeer?.let { viewModel.startFor(it) }
                requestCamera()
            }

            PairingScreen(
                state = state,
                onDigit = viewModel::onDigit,
                onBackspace = viewModel::onBackspace,
                onSubmit = viewModel::onSubmit,
                onQrScanned = viewModel::onQrScanned,
                onBack = {
                    viewModel.reset()
                    navController.popBackStack()
                },
            )
        }

        composable(Routes.STREAM) {
            val viewModel: StreamViewModel = viewModel(
                factory = StreamViewModel.factory(
                    client = container.client,
                    settingsRepo = container.settingsRepo,
                ),
            )
            val state by viewModel.state.collectAsStateWithLifecycle()

            LaunchedEffect(selectedPeer?.id) {
                selectedPeer?.let { viewModel.connect(it) }
            }

            StreamScreen(
                state = state,
                onSurfaceAvailable = viewModel::onSurfaceAvailable,
                onSurfaceDestroyed = viewModel::onSurfaceDestroyed,
                onToggleOverlay = viewModel::toggleOverlay,
                onToggleToolbar = viewModel::toggleToolbar,
                onDisconnect = {
                    viewModel.disconnect()
                    navController.popBackStack()
                },
                onInput = viewModel::sendInput,
            )
        }

        composable(Routes.SETTINGS) {
            val viewModel: SettingsViewModel = viewModel(
                factory = SettingsViewModel.factory(container.settingsRepo),
            )
            val state by viewModel.state.collectAsStateWithLifecycle()
            SettingsScreen(
                state = state,
                onBack = { navController.popBackStack() },
                onResolutionChange = viewModel::setResolution,
                onFpsChange = viewModel::setFps,
                onCodecChange = viewModel::setCodec,
                onBitrateChange = viewModel::setBitrate,
                onFecChange = viewModel::setFecRatio,
                onAudioChange = viewModel::setAudioEnabled,
                onOverlayChange = viewModel::setOverlayEnabled,
                onDecoderPreferenceChange = viewModel::setDecoderPreference,
                onThemeChange = viewModel::setThemeMode,
                onReset = viewModel::reset,
            )
        }

        composable(Routes.PERF) {
            val state by PerfSnapshotStore.state.collectAsStateWithLifecycle()
            Column(modifier = Modifier.fillMaxSize()) {
                Text(
                    text = "← 返回",
                    modifier = Modifier
                        .clickable { navController.popBackStack() }
                        .padding(12.dp),
                )
                dev.beam.app.perf.PerfEntry.Panel(
                    state = state,
                    onExport = { format: PerfExportFormat ->
                        dev.beam.app.perf.PerfEntry.export(context, format)
                    },
                )
            }
        }
    }
}
