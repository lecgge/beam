package dev.beam.feature.perf

import dev.beam.core.data.perf.PerfStageTiming
import kotlin.math.ceil

/**
 * 阶段耗时采样累加器（§3.13「各阶段耗时直方图 / 10 分钟环形缓冲」的客户端侧实现）。
 *
 * 纯 Kotlin，无 Android 依赖，方便单元测试。
 *
 * @param capacity 每个阶段保留的样本数；环形覆盖。
 */
class StageAccumulator(
    private val capacity: Int = DEFAULT_CAPACITY,
) {

    private val samples = LinkedHashMap<String, ArrayDeque<Double>>()

    /** 记录一次采样。 */
    fun record(stage: String, ms: Double) {
        val queue = samples.getOrPut(stage) { ArrayDeque() }
        queue.addLast(ms)
        while (queue.size > capacity) queue.removeFirst()
    }

    /** 各阶段的均值 / P99。 */
    fun timings(): List<PerfStageTiming> = samples.map { (stage, queue) ->
        val sorted = queue.toList().sorted()
        val mean = if (sorted.isEmpty()) 0.0 else sorted.sum() / sorted.size
        val p99 = if (sorted.isEmpty()) 0.0 else {
            val index = (ceil(sorted.size * 0.99).toInt() - 1).coerceIn(0, sorted.size - 1)
            sorted[index]
        }
        PerfStageTiming(stage = stage, meanMs = mean, p99Ms = p99)
    }

    /** 清空全部采样。 */
    fun clear() = samples.clear()

    companion object {
        /** 默认样本数：debug 250 ms 一拍 → 约 1 分钟历史。 */
        const val DEFAULT_CAPACITY: Int = 256
    }
}
