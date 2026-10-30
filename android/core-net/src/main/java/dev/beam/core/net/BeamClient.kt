package dev.beam.core.net

import kotlinx.coroutines.flow.Flow

/**
 * Beam 客户端（Android 端唯一的 Rust 入口）。
 *
 * 签名与 `beam-ffi` 的 uniffi 导出对齐：
 * ```rust
 * BeamClient::new(config)
 * client.browse(listener)
 * client.pair_qr(payload)
 * client.pair_pin(peer, pin)
 * client.connect(peer, cfg, sink: FrameSink)
 * ```
 *
 * **禁止事项**：本模块之外的任何模块都不得直接触碰 JNI / `System.loadLibrary`；
 * `:feature-discovery` 等界面层必须经由本接口访问 Rust 侧能力（§3.12 表）。
 */
interface BeamClient {

    /**
     * 局域网发现主机。
     *
     * 对应 `browse(listener)`：Rust 侧 mdns-sd 浏览结果经回调转换为 Flow。
     */
    fun discover(): Flow<DiscoveredHost>

    /**
     * 扫码配对。
     *
     * 对应 `pair_qr(payload)`，`payload` 形如 `beam://pair?h=ip:port&fp=..&t=token`。
     */
    suspend fun pairQr(payload: String): PairResult

    /**
     * PIN 配对（SPAKE2）。
     *
     * 对应 `pair_pin(peer, pin)`。失败时返回 [PairResult.failure]，错误码见 §3.3：
     * `E3001` PIN 错误、`E3002` 次数超限锁定 60 s、`E3003` token 过期、`E3004` 指纹不匹配。
     */
    suspend fun pairPin(peer: Peer, pin: String): PairResult

    /**
     * 建立串流会话。
     *
     * 对应 `connect(peer, cfg, sink: FrameSink)`；帧由 [sink] 回调下送。
     */
    suspend fun connect(peer: Peer, cfg: StreamCfg, sink: FrameSink): Session

    /**
     * 建立串流会话（**文档约定的二参签名**，§3.12 对外接口表）。
     *
     * 帧交给默认接收端 [DefaultFrameSink]：帧被计数后立即归还缓冲，不落屏。
     * 需要自己消费帧时请用带 sink 的重载。
     */
    suspend fun connect(peer: Peer, cfg: StreamCfg): Session

    /**
     * 建立串流会话（自带默认帧接收端）。
     *
     * §3.12 表格给出的对外签名形式；帧会交给 [sinkFactory] 产生的接收端。
     */
    suspend fun connect(peer: Peer, cfg: StreamCfg, sinkFactory: (StreamCfg) -> FrameSink): Session

    /** 释放底层运行时资源。 */
    fun close()
}

/** 配对结果。 */
sealed interface PairResult {

    /** 配对成功，[peer] 已带固定指纹。 */
    data class Success(val peer: Peer) : PairResult

    /** 配对失败。 */
    data class Failure(
        val code: String,
        val message: String,
        val retryable: Boolean,
        /** PIN 场景下的剩余尝试次数；`null` 表示不适用。 */
        val attemptsLeft: Int? = null,
        /** 锁定剩余毫秒（`E3002`）。 */
        val lockoutMs: Long? = null,
    ) : PairResult
}
