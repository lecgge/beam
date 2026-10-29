//! 解码线程 → 单槽 → 渲染线程 的单槽交换（§3.10 线程模型）。
//!
//! §3.10：「线程:解码线程 → 单槽 → 渲染线程(按 vblank)」。
//!
//! 单槽（single slot）是低延迟串流的经典做法：
//!
//! - **解码线程**把解好的画面写进唯一的槽位；如果槽位里还有一帧没被取走，
//!   说明渲染线程落后了，这一帧直接被**覆盖**（丢帧），避免延迟累积。
//! - **渲染线程**按 vblank 取走当前帧上屏。
//!
//! 槽位里保存的是 [`DecodedOutput`]：除了协议层的 [`DecodedFrame`]，
//! 还带着 D3D11 纹理的保活引用——渲染线程取走之前，纹理绝不能被释放。

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use beam_proto::DecodedFrame;

/// 解码输出 + 纹理保活句柄。
#[derive(Debug)]
pub struct DecodedOutput {
    /// 给渲染层的帧（含时间戳、尺寸与渲染目标句柄）。
    pub frame: DecodedFrame,
    /// D3D11 纹理的保活引用；`None` 表示这一帧没有硬件表面（例如软解回退）。
    pub surface: Option<crate::decoder::DecodedSurface>,
}

/// 单槽内部状态。
#[derive(Debug, Default)]
struct SlotState {
    /// 当前待取走的帧。
    entry: Option<DecodedOutput>,
    /// 已发布的帧数。
    published: u64,
    /// 被覆盖（过时）的帧数——即 §3.10 说的「丢帧统计」。
    dropped: u64,
    /// 已被渲染线程取走的帧数。
    consumed: u64,
}

/// 解码线程 → 渲染线程的单槽。
///
/// 线程安全：内部用互斥量保护，渲染线程可以阻塞等待新帧。
#[derive(Debug)]
pub struct FrameSlot {
    state: Mutex<SlotState>,
    /// 新帧到达时唤醒等待中的渲染线程。
    ready: Condvar,
}

impl Default for FrameSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSlot {
    /// 新建一个空槽。
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SlotState::default()),
            ready: Condvar::new(),
        }
    }

    /// 解码线程：发布最新一帧。
    ///
    /// 若槽位里还有一帧没被取走，会被这一帧**覆盖**（计入丢帧统计）。
    pub fn publish(&self, entry: DecodedOutput) {
        let mut state = self.lock();
        if state.entry.is_some() {
            state.dropped += 1;
        }
        state.entry = Some(entry);
        state.published += 1;
        drop(state);
        self.ready.notify_one();
    }

    /// 渲染线程：取走当前帧（没有则立即返回 `None`，不阻塞）。
    pub fn take(&self) -> Option<DecodedOutput> {
        let mut state = self.lock();
        let entry = state.entry.take();
        if entry.is_some() {
            state.consumed += 1;
        }
        entry
    }

    /// 渲染线程：等待一帧，最多等 `timeout`。
    ///
    /// 用于「按 vblank 上屏」的渲染循环：等不到就跳过这一拍。
    pub fn take_timeout(&self, timeout: Duration) -> Option<DecodedOutput> {
        let mut state = self.lock();
        if state.entry.is_none() {
            let (guard, _timed_out) = self
                .ready
                .wait_timeout(state, timeout)
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
        }
        let entry = state.entry.take();
        if entry.is_some() {
            state.consumed += 1;
        }
        entry
    }

    /// 已发布的帧数。
    pub fn published(&self) -> u64 {
        self.lock().published
    }

    /// 被覆盖（过时）的帧数（§3.10 的丢帧统计）。
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    /// 已被渲染线程取走的帧数。
    pub fn consumed(&self) -> u64 {
        self.lock().consumed
    }

    /// 槽位里是否还有待取走的帧。
    pub fn is_pending(&self) -> bool {
        self.lock().entry.is_some()
    }

    /// 取锁；锁中毒时退回未 poisoning 的锁，避免整个渲染循环被打断。
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_proto::PlatformHandle;

    fn output(frame_id: u32) -> DecodedOutput {
        DecodedOutput {
            frame: DecodedFrame {
                frame_id,
                pts_us: frame_id as u64,
                capture_ts_us: frame_id as u64,
                width: 1920,
                height: 1080,
                is_key: true,
                surface: PlatformHandle::NONE,
            },
            surface: None,
        }
    }

    #[test]
    fn publish_then_take() {
        let slot = FrameSlot::new();
        assert!(slot.take().is_none());
        slot.publish(output(1));
        assert!(slot.is_pending());
        let got = slot.take().expect("槽位里应有帧");
        assert_eq!(got.frame.frame_id, 1);
        assert_eq!(slot.published(), 1);
        assert_eq!(slot.consumed(), 1);
        assert_eq!(slot.dropped(), 0);
    }

    #[test]
    fn newest_frame_wins_and_older_is_dropped() {
        let slot = FrameSlot::new();
        slot.publish(output(1));
        slot.publish(output(2));
        assert_eq!(slot.dropped(), 1, "旧的帧应计入丢帧统计");
        let got = slot.take().expect("槽位里应有帧");
        assert_eq!(got.frame.frame_id, 2, "单槽只保留最新帧");
    }

    #[test]
    fn take_timeout_returns_none_when_idle() {
        let slot = FrameSlot::new();
        assert!(slot.take_timeout(Duration::from_millis(20)).is_none());
    }

    #[test]
    fn take_timeout_wakes_on_publish() {
        let slot = std::sync::Arc::new(FrameSlot::new());
        let writer = std::sync::Arc::clone(&slot);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            writer.publish(output(7));
        });
        let got = slot.take_timeout(Duration::from_secs(5)).expect("应等到帧");
        assert_eq!(got.frame.frame_id, 7);
        handle.join().expect("写线程不应 panic");
    }
}
