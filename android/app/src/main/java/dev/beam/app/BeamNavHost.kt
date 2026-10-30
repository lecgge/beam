package dev.beam.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import dev.beam.app.perf.PerfEntry
import dev.beam.app.stream.StreamForegroundService
import dev.beam.core.data.perf.PerfExportFormat
import dev.beam.core.data.perf.PerfSnapshotStore
import dev.beam.core.data.ui.BeamToast
import dev.beam.feature.discovery.DiscoveryScreen
import dev.beam.feature.discovery.DiscoveryViewModel
import dev.beam.feature.discovery.NsdHostScanner
import dev.beam.feature.pairing.PairingScreen
import dev.beam.feature.pairing.PairingViewModel
import dev.beam.feature.settings.SettingsScreen
import dev.beam.feature.settings.SettingsViewModel
import dev.beam.feature.stream.StreamPhase
import dev.beam.feature.stream.StreamScreen
import dev.beam.feature.stream.StreamViewModel
import kotlinx.coroutines.launch

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
    val scope = rememberCoroutineScope()
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
                // 二次确认已在页面内用模态弹窗完成，这里只负责落地。
                onRemoveTrust = { row ->
                    scope.launch {
                        val ok = viewModel.removeTrust(row)
                        BeamToast.show(if (ok) "已移除信任" else "移除失败，请重试")
                    }
                },
                // 空态的「手动输入 IP」：地址非法时把错误留在页面上。
                onManualConnect = { address ->
                    val peer = viewModel.manualPeer(address)
                    if (peer == null) {
                        viewModel.reportError(
                            "地址格式不正确，请输入 ip 或 ip:port（例如 192.168.1.10:4827）",
                        )
                    } else {
                        container.selectedPeer.value = peer
                        navController.navigate(Routes.PAIRING)
                    }
                },
                onCopyAddress = { row ->
                    copyToClipboard(context, row.peer.address)
                    BeamToast.show("已复制地址")
                },
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
            val requestNotifications = rememberPermissionRequest(BeamPermissions.POST_NOTIFICATIONS)

            LaunchedEffect(selectedPeer?.id) {
                selectedPeer?.let { viewModel.connect(it) }
            }

            // 串流前台服务 + 性能采集（AN-52 / AN-23）。
            //
            // 两者都跟着**会话**走：串流开始时起，串流结束时停；
            // 性能面板的快照留在 `PerfSnapshotStore` 里，离开串流页后仍可查看上一次会话的真实数据。
            LaunchedEffect(state.phase) {
                when (state.phase) {
                    StreamPhase.STREAMING -> {
                        // Android 13+ 的通知运行时权限：没申请过就顺手申请。
                        requestNotifications()
                        StreamForegroundService.start(context, state.peer?.name.orEmpty())
                        PerfEntry.startRecording(container.appScope, viewModel.sessionStats)
                    }
                    StreamPhase.DISCONNECTED, StreamPhase.ERROR -> {
                        StreamForegroundService.stop(context)
                        PerfEntry.stopRecording()
                    }
                    else -> Unit
                }
            }

            DisposableEffect(Unit) {
                onDispose {
                    // 离开串流页必须停掉前台服务（否则通知会一直挂着）。
                    StreamForegroundService.stop(context)
                }
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
                onReset = {
                    scope.launch {
                        val ok = viewModel.reset()
                        BeamToast.show(if (ok) "已恢复默认设置" else "恢复默认设置失败")
                    }
                },
                // 性能面板入口（AN-23）：仅 debug 构建可达。
                onOpenPerf = if (dev.beam.app.perf.PerfEntry.available) {
                    { navController.navigate(Routes.PERF) }
                } else {
                    null
                },
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
                        val result = dev.beam.app.perf.PerfEntry.export(context, format)
                        BeamToast.show(
                            when {
                                result.isBlank() -> "当前构建不支持导出"
                                result.startsWith("导出失败") -> result
                                else -> "已导出 ${format.name}"
                            },
                        )
                    },
                )
            }
        }
    }
}

/** 复制文本到剪贴板。 */
private fun copyToClipboard(context: Context, text: String) {
    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText("Beam 主机地址", text))
}
