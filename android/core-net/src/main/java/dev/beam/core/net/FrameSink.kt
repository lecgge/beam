package dev.beam.core.net

/**
 * 帧接收端。
 *
 * 对齐 `beam-ffi` 的 uniffi 导出：
 * ```rust
 * callback interface FrameSink {
 *     fn on_frame(data: ByteBuffer, pts_us: u64, capture_ts_us: u64, key: bool);
 *     fn on_event(event: SessionEvent);
 * }
 * ```
 * 帧数据走 JNI DirectByteBuffer（热路径），事件走 uniffi（控制面）。
 *
 * **线程**：回调发生在 Rust tokio 侧派发的专用线程，不在主线程；实现方不得阻塞。
 */
interface FrameSink {

    /**
     * 收到一帧压缩视频数据。
     *
     * @param buffer 直接内存缓冲；实现方若需异步持有，必须自行复制或取得池化租约。
     * @param ptsUs 单调时钟微秒时间戳（§6 约定）
     * @param captureTsUs 主机抓屏时刻（微秒），用于 E2E 计算
     * @param key 是否为关键帧（IDR）
     */
    fun onFrame(buffer: java.nio.ByteBuffer, ptsUs: Long, captureTsUs: Long, key: Boolean)

    /** 收到控制面事件。 */
    fun onEvent(event: SessionEvent)
}
