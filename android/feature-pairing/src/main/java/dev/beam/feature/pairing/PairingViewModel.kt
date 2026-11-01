package dev.beam.feature.pairing

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import dev.beam.core.data.HostRepo
import dev.beam.core.data.TrustedHost
import dev.beam.core.net.BeamClient
import dev.beam.core.net.PairResult
import dev.beam.core.net.Peer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * 配对阶段。
 */
enum class PairingPhase {
    /** 等待进入配对。 */
    IDLE,

    /** 相机扫码中。 */
    SCANNING,

    /** 输入 PIN。 */
    PIN_ENTRY,

    /** 正在与主机握手。 */
    PAIRING,

    /** 配对成功。 */
    SUCCESS,

    /** 配对失败。 */
    FAILED,
}

/**
 * 配对页状态。
 */
data class PairingUiState(
    /** 目标主机（扫码或从列表进入时确定）。 */
    val peer: Peer? = null,
    /** 当前阶段。 */
    val phase: PairingPhase = PairingPhase.IDLE,
    /** 已输入的 PIN。 */
    val pin: String = "",
    /** PIN 位数。 */
    val pinLength: Int = DEFAULT_PIN_LENGTH,
    /** 错误文案（已按错误码翻译）。 */
    val error: String? = null,
    /** 剩余尝试次数（§3.3 `E3002` 之后显示）。 */
    val attemptsLeft: Int? = null,
    /** 锁定截止时间（墙钟毫秒）。 */
    val lockoutUntilMs: Long? = null,
) {
    /** PIN 是否已填满。 */
    val isPinComplete: Boolean get() = pin.length == pinLength

    companion object {
        /** 默认 PIN 位数（§3.3 `pin_len=4`，产品方案允许 4–6 位）。 */
        const val DEFAULT_PIN_LENGTH: Int = 4
    }
}

/**
 * 配对 ViewModel（§3.12 `:feature-pairing` 的对外接口）。
 *
 * 支持两条路径：
 * - **扫码**：CameraX + ML Kit 解析 `beam://pair?...`，调 `pairQr`；
 * - **PIN**：手工输入 4–6 位数字，调 `pairPin`（SPAKE2，§3.3）。
 *
 * 错误码：§3.3 `E3001` PIN 错误、`E3002` 次数超限锁定 60 s、`E3003` token 过期、
 * `E3004` 指纹不匹配。
 */
class PairingViewModel(
    private val client: BeamClient,
    private val hostRepo: HostRepo,
    private val clock: () -> Long = System::currentTimeMillis,
) : ViewModel() {

    private val _state = MutableStateFlow(PairingUiState())
    val state: StateFlow<PairingUiState> = _state.asStateFlow()

    /** 进入配对流程（从主机列表点进来）。 */
    fun startFor(peer: Peer) {
        _state.value = PairingUiState(
            peer = peer,
            phase = if (peer.trusted) PairingPhase.SCANNING else PairingPhase.PIN_ENTRY,
        )
    }

    /** 进入扫码模式。 */
    fun startScanning() {
        _state.update { it.copy(phase = PairingPhase.SCANNING, error = null) }
    }

    /** 扫码命中：解析载荷并发起配对。 */
    fun onQrScanned(payload: String) {
        val parsed = PairPayload.parse(payload)
        if (parsed == null) {
            _state.update {
                it.copy(phase = PairingPhase.FAILED, error = "二维码内容不是有效的 Beam 配对载荷")
            }
            return
        }
        val peer = Peer(
            id = parsed.address,
            name = parsed.address,
            address = parsed.address,
            fingerprint = parsed.fingerprint,
            // 二维码载荷（§3.3）不带平台字段 ⇒ 如实记「未知平台」。
            platform = Peer.PLATFORM_UNKNOWN,
            trusted = false,
        )
        _state.update { it.copy(peer = peer, phase = PairingPhase.PAIRING, error = null) }
        viewModelScope.launch {
            when (val result = client.pairQr(payload)) {
                is PairResult.Success -> onPaired(result.peer)
                is PairResult.Failure -> onFailure(result)
            }
        }
    }

    /** 追加一位 PIN。 */
    fun onDigit(digit: Int) {
        if (digit !in 0..9) return
        _state.update { current ->
            if (current.pin.length >= current.pinLength) return@update current
            current.copy(pin = current.pin + digit, error = null)
        }
    }

    /** 删除一位 PIN。 */
    fun onBackspace() {
        _state.update { it.copy(pin = it.pin.dropLast(1), error = null) }
    }

    /** 提交 PIN。 */
    fun onSubmit() {
        val current = _state.value
        val peer = current.peer
        if (peer == null || !current.isPinComplete) return
        if (current.lockoutUntilMs?.let { it > clock() } == true) return

        _state.update { it.copy(phase = PairingPhase.PAIRING, error = null) }
        viewModelScope.launch {
            when (val result = client.pairPin(peer, current.pin)) {
                is PairResult.Success -> onPaired(result.peer)
                is PairResult.Failure -> onFailure(result)
            }
        }
    }

    /** 重置（返回列表）。 */
    fun reset() {
        _state.value = PairingUiState()
    }

    private suspend fun onPaired(peer: Peer) {
        hostRepo.upsert(TrustedHost(peer = peer.copy(trusted = true), pairedAtMs = clock()))
        _state.update { it.copy(peer = peer.copy(trusted = true), phase = PairingPhase.SUCCESS, pin = "") }
    }

    private fun onFailure(failure: PairResult.Failure) {
        _state.update { current ->
            current.copy(
                phase = PairingPhase.FAILED,
                error = describe(failure),
                pin = if (failure.code == CODE_PIN_WRONG) "" else current.pin,
                attemptsLeft = failure.attemptsLeft,
                lockoutUntilMs = failure.lockoutMs?.let { clock() + it },
            )
        }
    }

    private fun describe(failure: PairResult.Failure): String = when (failure.code) {
        CODE_PIN_WRONG -> "PIN 错误，请核对主机屏幕上的数字" +
            (failure.attemptsLeft?.let { "（剩余 $it 次）" } ?: "")
        CODE_LOCKED -> "尝试次数过多，已锁定，请稍后再试"
        CODE_TOKEN_EXPIRED -> "二维码已过期，请在主机上刷新后重新扫码"
        CODE_FINGERPRINT_MISMATCH -> "主机指纹与已信任的主机不一致，已拒绝连接"
        else -> failure.message.ifBlank { "配对失败（${failure.code}）" }
    }

    companion object {
        /** §3.3 错误码。 */
        const val CODE_PIN_WRONG: String = "E3001"
        const val CODE_LOCKED: String = "E3002"
        const val CODE_TOKEN_EXPIRED: String = "E3003"
        const val CODE_FINGERPRINT_MISMATCH: String = "E3004"

        /** 手动构造函数注入用的工厂（不引入 DI 框架）。 */
        fun factory(
            client: BeamClient,
            hostRepo: HostRepo,
            clock: () -> Long = System::currentTimeMillis,
        ): ViewModelProvider.Factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T =
                PairingViewModel(client, hostRepo, clock) as T
        }
    }
}
