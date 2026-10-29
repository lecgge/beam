package dev.beam.core.input

/**
 * 输入事件时间戳来源。
 *
 * 协议侧约定用**单调时钟微秒**（`System.nanoTime()`），
 * 不能用墙钟（`currentTimeMillis`）—— 墙钟会被用户或 NTP 调整。
 */
object InputClock {

    /** 当前单调时钟（微秒）。 */
    fun nowUs(): Long = System.nanoTime() / 1000
}
