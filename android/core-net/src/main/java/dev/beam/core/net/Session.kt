package dev.beam.core.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow

/**
 * 一次串流会话。
 *
 * 对应 Rust 侧 `beam-session::SessionHandle`：`events()->Stream<SessionEvent>`、
 * `update(StreamConfig)`、`stop()`。
 */
interface Session {

    /** 会话状态。 */
    val state: StateFlow<SessionState>

    /** 控制面事件流（对应 §3.11 的 events 清单）。 */
    val events: Flow<SessionEvent>

    /** 统计流，`session://stats` 的 500 ms 快照。 */
    val stats: Flow<SessionStats>

    /** 音频包流（v1 音频阶段；未启用音频时为空流）。 */
    val audioPackets: Flow<AudioPacket>

    /**
     * 当前的重连信息；**不在**重连态时返回 `null`（§4.1）。
     *
     * 上层据此显示「重连中」并**保留最后一帧**（`keepLastFrame = true`）。
     */
    fun reconnecting(): ReconnectingInfo?

    /**
     * 会话汇总（§3.9）：帧数 / 丢帧 / RTT / 丢包率 / 帧率 / 码率 / 重连次数。
     *
     * 与 [stats] 的区别：[stats] 是会话内的周期指标流，
     * 本方法是**会话结束后依然可查**的累计值。
     */
    fun summary(): SessionSummary?

    /** 下发输入事件（键鼠/触控/手柄）。 */
    fun sendInput(event: InputEvent)

    /** 请求主机发送新的 IDR。 */
    fun requestIdr()

    /** 运行时更新串流配置（分辨率/帧率/码率）。 */
    fun update(cfg: StreamCfg)

    /** 主动断开并释放资源。 */
    fun close()
}
