package dev.beam.core.net

import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicLong

/**
 * 默认帧接收端：帧被**计数后立即归还**给缓冲池，不落屏、不排队。
 *
 * 用途：`BeamClient.connect(peer, cfg)`（§3.12 表格里的二参签名）用它作为默认
 * 接收端 —— 调用方只想「建立会话、拿统计」，不想自己处理帧。
 *
 * 实现 [PooledFrameSink] 表示**接管缓冲所有权**：桥接层在回调后不再归还缓冲，
 * 由本类负责 `close()` 归还（§6 内存所有权约定）。
 */
class DefaultFrameSink : PooledFrameSink {

    private val frameCount = AtomicLong(0L)
    private val keyFrameCount = AtomicLong(0L)

    /** 已收到的帧数。 */
    val frames: Long get() = frameCount.get()

    /** 已收到的关键帧（IDR）数。 */
    val keyFrames: Long get() = keyFrameCount.get()

    override fun onPooledFrame(frame: PooledFrame) {
        frameCount.incrementAndGet()
        if (frame.key) keyFrameCount.incrementAndGet()
        // 立刻归还缓冲给池：默认接收端不持有帧。
        frame.close()
    }

    override fun onFrame(buffer: ByteBuffer, ptsUs: Long, captureTsUs: Long, key: Boolean) {
        frameCount.incrementAndGet()
        if (key) keyFrameCount.incrementAndGet()
    }

    override fun onEvent(event: SessionEvent) = Unit
}
