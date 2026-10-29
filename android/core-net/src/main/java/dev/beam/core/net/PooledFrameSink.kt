package dev.beam.core.net

import java.nio.ByteBuffer

/**
 * 可选地承接「池化帧所有权」的接收端。
 *
 * 普通 [FrameSink] 的语义是：`onFrame` 返回后缓冲即失效（§6：DirectByteBuffer
 * 由 Rust 侧池化，回调返回后归还）。若消费方需要异步持有该帧（例如把帧放进
 * 自己的队列），实现本接口即可接管所有权——桥接层不再归还缓冲，由消费方在
 * 消费完毕后 `PooledFrame.close()`。
 */
interface PooledFrameSink : FrameSink {

    /**
     * 收到一帧压缩视频数据，并**接管缓冲所有权**。
     *
     * 实现方必须在消费完毕后调用 [PooledFrame.close] 把缓冲归还给池。
     */
    fun onPooledFrame(frame: PooledFrame)
}
