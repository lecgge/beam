package dev.beam.feature.discovery

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import dev.beam.core.data.HostRepo
import dev.beam.core.net.BeamClient
import dev.beam.core.net.DiscoveredHost
import dev.beam.core.net.DiscoverySource
import dev.beam.core.net.Peer
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * 主机列表行。
 */
data class HostRow(
    val peer: Peer,
    val source: DiscoverySource,
    val trusted: Boolean,
    val lastSeenMs: Long,
)

/**
 * 主机列表页状态。
 */
data class DiscoveryUiState(
    /** 是否正在扫描。 */
    val isScanning: Boolean = false,
    /** 已发现 / 已信任的主机。 */
    val hosts: List<HostRow> = emptyList(),
    /** 错误提示（已按错误码翻译成中文文案）。 */
    val error: String? = null,
    /** Rust 原生库是否就位；false 时提示「尚未构建 so」。 */
    val nativeReady: Boolean = true,
) {
    /** 是否为空态。 */
    val isEmpty: Boolean get() = hosts.isEmpty() && !isScanning
}

/**
 * 主机列表 ViewModel（§3.12 `:feature-discovery` 的对外接口）。
 *
 * ## 禁止事项
 * **禁止直接调 JNI**：所有 Rust 能力都经由 `:core-net` 的 [BeamClient]。
 *
 * ## 状态
 * 用 [StateFlow] 暴露不可变状态；UI 只读状态并回调方法，不持有业务逻辑。
 */
class DiscoveryViewModel(
    private val client: BeamClient,
    private val hostRepo: HostRepo,
    private val fallbackScanner: HostScanner?,
    private val nativeReady: Boolean = true,
) : ViewModel() {

    private val _state = MutableStateFlow(DiscoveryUiState(nativeReady = nativeReady))
    val state: StateFlow<DiscoveryUiState> = _state.asStateFlow()

    private var scanJob: Job? = null

    init {
        observeTrustedHosts()
    }

    /** 开始扫描（Rust mDNS 为主，NsdManager 兜底）。 */
    fun start() {
        if (scanJob?.isActive == true) return
        _state.update { it.copy(isScanning = true, error = null) }
        scanJob = viewModelScope.launch {
            val merged = LinkedHashMap<String, HostRow>()
            launch {
                client.discover()
                    .catch { cause ->
                        _state.update { it.copy(error = describe(cause)) }
                    }
                    .collect { host -> merge(merged, host) }
            }
            fallbackScanner?.let { scanner ->
                launch {
                    scanner.scan()
                        .catch { /* 兜底失败不影响主路径 */ }
                        .collect { host -> merge(merged, host) }
                }
            }
            _state.update { it.copy(isScanning = false) }
        }
    }

    /** 停止扫描。 */
    fun stop() {
        scanJob?.cancel()
        scanJob = null
        _state.update { it.copy(isScanning = false) }
    }

    /** 重新扫描。 */
    fun refresh() {
        stop()
        start()
    }

    /**
     * 移除对某个主机的信任。
     *
     * **调用方负责二次确认**（UI 弹模态确认框），这里只负责落地。
     *
     * @return 是否移除成功；失败时调用方应如实报错，而不是提示「已移除」。
     */
    suspend fun removeTrust(row: HostRow): Boolean {
        val fingerprint = row.peer.fingerprint
        val removed = if (fingerprint.isNotBlank()) {
            runCatching { hostRepo.remove(fingerprint) }.isSuccess
        } else {
            true
        }
        if (removed) {
            _state.update { current ->
                current.copy(hosts = current.hosts.filterNot { it.peer.id == row.peer.id })
            }
        }
        return removed
    }

    /**
     * 校验手动输入的主机地址，合法时返回主机对象。
     *
     * 接受 `ip` 与 `ip:port` 两种写法，缺省端口 [ManualAddress.DEFAULT_PORT]。
     * 非法时返回 null（由调用方在页面上提示错误）。
     */
    fun manualPeer(input: String): Peer? = ManualAddress.parse(input)

    /** 报告一条错误提示（留在页面上，不弹窗）。 */
    fun reportError(message: String) {
        _state.update { it.copy(error = message) }
    }

    private fun merge(target: MutableMap<String, HostRow>, host: DiscoveredHost) {
        val key = host.peer.id.ifBlank { host.peer.address }
        val trusted = _state.value.hosts
            .firstOrNull { it.peer.id == key || it.peer.fingerprint == host.peer.fingerprint }
            ?.trusted ?: false
        target[key] = HostRow(
            peer = host.peer.copy(trusted = trusted),
            source = host.discoveredBy,
            trusted = trusted,
            lastSeenMs = host.lastSeenMs,
        )
        _state.update { current ->
            current.copy(hosts = target.values.sortedBy { it.peer.name })
        }
    }

    private fun observeTrustedHosts() {
        viewModelScope.launch {
            hostRepo.hosts.collect { trusted ->
                _state.update { current ->
                    current.copy(
                        hosts = current.hosts.map { row ->
                            val hit = trusted.firstOrNull { it.peer.fingerprint == row.peer.fingerprint }
                            row.copy(
                                trusted = hit != null,
                                peer = row.peer.copy(trusted = hit != null),
                            )
                        },
                    )
                }
            }
        }
    }

    private fun describe(cause: Throwable): String = when (cause) {
        is dev.beam.core.net.BeamNativeUnavailableException ->
            "原生库尚未构建：请按 core-net/README.md 生成 uniffi 绑定与 libbeam_ffi.so"
        else -> cause.message ?: "发现主机失败"
    }

    companion object {

        /**
         * 手动构造函数注入用的工厂（不引入 DI 框架）。
         */
        fun factory(
            client: BeamClient,
            hostRepo: HostRepo,
            fallbackScanner: HostScanner?,
            nativeReady: Boolean = true,
        ): ViewModelProvider.Factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                DiscoveryViewModel(client, hostRepo, fallbackScanner, nativeReady) as T
        }
    }
}
