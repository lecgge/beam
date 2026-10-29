package dev.beam.app

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.preferencesDataStoreFile
import dev.beam.core.data.HostRepo
import dev.beam.core.data.SettingsRepo
import dev.beam.core.net.BeamClient
import dev.beam.core.net.NativeBeamBridge
import dev.beam.core.net.Peer
import dev.beam.core.net.UnavailableBeamClient
import kotlinx.coroutines.flow.MutableStateFlow

/**
 * 应用容器：**手动构造函数注入**（不使用 DI 框架）。
 *
 * 依赖关系：
 * ```
 * DataStore ──┬── SettingsRepo
 *              └── HostRepo
 * NativeBeamBridge ── BeamClient
 * ```
 */
class AppContainer(context: Context) {

    private val appContext: Context = context.applicationContext

    private val dataStore: DataStore<Preferences> = PreferenceDataStoreFactory.create {
        appContext.preferencesDataStoreFile(DATASTORE_NAME)
    }

    /** 设置仓库。 */
    val settingsRepo: SettingsRepo = SettingsRepo(dataStore)

    /** 信任主机仓库。 */
    val hostRepo: HostRepo = HostRepo(dataStore)

    /** 原生库与 uniffi 绑定是否就位。 */
    val nativeReady: Boolean = NativeBeamBridge.isAvailable()

    /**
     * Rust 客户端。
     *
     * 生成物缺席时降级为 [UnavailableBeamClient]，UI 会显示明确原因而不是崩溃。
     */
    val client: BeamClient = if (nativeReady) {
        runCatching { NativeBeamBridge.create() }
            .getOrElse { UnavailableBeamClient(it.message ?: UnavailableBeamClient.DEFAULT_REASON) }
    } else {
        UnavailableBeamClient()
    }

    /** 当前选中的主机（页面之间传递用，避免用路由参数编码复杂对象）。 */
    val selectedPeer = MutableStateFlow<Peer?>(null)

    companion object {
        /** DataStore 文件名。 */
        const val DATASTORE_NAME: String = "beam_settings"
    }
}
