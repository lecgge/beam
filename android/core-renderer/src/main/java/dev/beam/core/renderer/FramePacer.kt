package dev.beam.core.renderer

/**
 * 帧节奏决策。
 *
 * 参见 [FramePacer]。
 */
sealed interface PacerDecision {

    /** 应当上屏。 */
    data object Present : PacerDecision

    /**
     * 应当丢帧（落后节拍太多，继续渲染只会加大延迟）。
     *
     * @param lateByUs 落后目标节拍多少微秒。
     */
    data class Drop(val lateByUs: Long) : PacerDecision

    /**
     * 应当跳过（乱序或重复帧）。
     *
     * @param reason 原因说明（仅用于日志/统计）。
     */
    data class Skip(val reason: String) : PacerDecision

    /**
     * 还没到上屏时刻。
     *
     * @param dueInUs 距离目标时刻还有多少微秒。
     */
    data class Wait(val dueInUs: Long) : PacerDecision
}

/**
 * 帧节奏控制器（§3.12 `:core-renderer` 的对外接口）。
 *
 * ## 口径说明（重要）
 *
 * `ptsUs` 来自**主机**的单调时钟（§6 约定），与客户端本地时钟**不同源**，
 * 因此两者不能直接相减。本控制器因此：
 *
 * - 用 [arriveUs]（帧到达客户端的单调时刻）驱动节拍；
 * - 用 [ptsUs] 只做**顺序校验**（丢弃乱序 / 重复帧）。
 *
 * ## 策略
 * - 迟到超过一个帧间隔 → 丢帧（[PacerDecision.Drop]），避免延迟累积；
 * - 提前到达 → 等待到节拍（[PacerDecision.Wait]）；
 * - 按时到达 → 上屏（[PacerDecision.Present]）。
 *
 * ## 线程模型
 * 非线程安全；由帧回调线程单线程调用。
 */
class FramePacer(
    /** 目标帧率。 */
    val targetFps: Int,
    /** 落后多少微秒后丢帧；默认一个帧间隔。 */
    private val maxLatenessUs: Long = defaultMaxLateness(targetFps),
) {

    init {
        require(targetFps > 0) { "目标帧率必须为正数" }
    }

    /** 目标帧间隔（微秒）。 */
    val frameIntervalUs: Long = 1_000_000L / targetFps

    private var nextDueUs: Long = UNSET
    private var lastPtsUs: Long = UNSET

    /**
     * 判断一帧是否应当上屏。
     *
     * @param ptsUs   帧的显示时间戳（主机单调时钟微秒）。
     * @param arriveUs 帧到达客户端的单调时刻（微秒）。
     */
    fun decide(ptsUs: Long, arriveUs: Long): PacerDecision {
        if (lastPtsUs != UNSET && ptsUs <= lastPtsUs) {
            return PacerDecision.Skip("pts 非递增")
        }
        lastPtsUs = ptsUs

        if (nextDueUs == UNSET) nextDueUs = arriveUs
        val lateByUs = arriveUs - nextDueUs
        return when {
            lateByUs > maxLatenessUs -> {
                nextDueUs += frameIntervalUs
                PacerDecision.Drop(lateByUs)
            }
            lateByUs < 0L -> PacerDecision.Wait(-lateByUs)
            else -> {
                nextDueUs += frameIntervalUs
                PacerDecision.Present
            }
        }
    }

    /** 重置节拍（重新建流或分辨率变化时调用）。 */
    fun reset() {
        nextDueUs = UNSET
        lastPtsUs = UNSET
    }

    /** 当前的目标节拍时刻。 */
    val dueUs: Long get() = nextDueUs

    companion object {
        private const val UNSET = Long.MIN_VALUE

        /** 默认丢帧阈值：一个帧间隔。 */
        fun defaultMaxLateness(targetFps: Int): Long =
            if (targetFps <= 0) 0L else 1_000_000L / targetFps
    }
}
