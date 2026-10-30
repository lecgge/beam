package dev.beam.core.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow

/**
 * 「原生库尚未构建」时的降级实现。
 *
 * 让 App 在没有 `libbeam_ffi.so` 的环境里也能启动，并把原因如实显示在 UI 上，
 * 而不是启动即崩溃。
 */
class UnavailableBeamClient(
    private val reason: String = DEFAULT_REASON,
) : BeamClient {

    override fun discover(): Flow<DiscoveredHost> = emptyFlow()

    override suspend fun pairQr(payload: String): PairResult = failure()

    override suspend fun pairPin(peer: Peer, pin: String): PairResult = failure()

    override suspend fun connect(peer: Peer, cfg: StreamCfg, sink: FrameSink): Session = failure()

    override suspend fun connect(peer: Peer, cfg: StreamCfg): Session = failure()

    override suspend fun connect(peer: Peer, cfg: StreamCfg, sinkFactory: (StreamCfg) -> FrameSink): Session =
        failure()

    override fun close() = Unit

    private fun failure(): Nothing = throw BeamNativeUnavailableException(reason)

    companion object {
        /** 默认提示文案。 */
        const val DEFAULT_REASON: String =
            "原生库 libbeam_ffi.so 尚未构建。请按 core-net/README.md 生成 uniffi 绑定并用 cargo-ndk 构建 .so。"
    }
}
