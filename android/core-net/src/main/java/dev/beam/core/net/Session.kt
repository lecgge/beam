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

    /** 下发输入事件（键鼠/触控/手柄）。 */
    fun sendInput(event: InputEvent)

    /** 请求主机发送新的 IDR。 */
    fun requestIdr()

    /** 运行时更新串流配置（分辨率/帧率/码率）。 */
    fun update(cfg: StreamCfg)

    /** 主动断开并释放资源。 */
    fun close()
}
