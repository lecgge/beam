package dev.beam.core.net

/**
 * 会话事件。
 *
 * 与技术设计文档 §3.11 的事件清单一一对应（Android 端消费同一组语义）：
 *  - `session://stats`（500 ms）→ [Stats]
 *  - `session://reconnecting` → [Reconnecting]
 *  - `session://disconnected` → [Disconnected]
 *  - `session://state-changed` → [StateChanged]
 *  - 错误统一为 `SessionEvent::Error{code, retryable}`（§6 错误传播）
 */
sealed interface SessionEvent {

    /** `session://stats`：500 ms 一次的统计快照。 */
    data class Stats(val stats: SessionStats) : SessionEvent

    /** `session://reconnecting`：进入重连。 */
    data class Reconnecting(
        val attempt: Int,
        val reason: String? = null,
        val elapsedMs: Long = 0L,
    ) : SessionEvent

    /** `session://disconnected`：会话已结束。 */
    data class Disconnected(
        val code: String,
        val message: String,
        val retryable: Boolean,
    ) : SessionEvent

    /** 会话状态机迁移，对应 §4.1 的状态集合。 */
    data class StateChanged(val state: SessionState, val previous: SessionState) : SessionEvent

    /** 统一错误出口；UI 只根据 [code] 显示文案。 */
    data class Error(
        val code: String,
        val message: String,
        val retryable: Boolean,
    ) : SessionEvent
}

/** 会话状态机（§3.9 beam-session）。 */
enum class SessionState {
    IDLE,
    DISCOVERING,
    PAIRING,
    CONNECTING,
    NEGOTIATING,
    STREAMING,
    RECONNECTING,
    CLOSED,
    ERROR,
}
