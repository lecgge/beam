//! 单槽帧交换（§3.10 线程模型 / §6 热路径约定）。
//!
//! §3.10 的线程模型是「解码线程 → 单槽 → 渲染线程（按 vblank）」：
//! 解码线程把最新的解码帧覆盖式写进单槽，渲染线程取走当前帧去上屏。
//!
//! §6 要求热路径不使用阻塞锁，因此这里用原子指针实现**无锁单槽**：
//! 交换本身是原子的，生产者与消费者各自最多拿到一份所有权。

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// 无锁单槽：发布 / 取走「最新一帧」。
pub(crate) struct FrameSlot<T: Send> {
    /// 当前帧的堆指针；为空表示没有待取帧。
    current: AtomicPtr<T>,
}

impl<T: Send> Default for FrameSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send> FrameSlot<T> {
    /// 新建一个空槽。
    pub(crate) const fn new() -> Self {
        Self {
            current: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// 发布最新一帧（覆盖旧帧）。
    ///
    /// 旧帧如果还没被取走，就在这里释放——这正是「单槽」语义：
    /// 渲染线程永远只拿到最新的一帧，不会积压延迟。
    ///
    /// 这里需要 unsafe：无锁单槽用裸堆指针在解码/渲染之间转移所有权。
    #[allow(unsafe_code)]
    pub(crate) fn publish(&self, value: T) {
        let fresh = Box::into_raw(Box::new(value));
        let stale = self.current.swap(fresh, Ordering::AcqRel);
        if !stale.is_null() {
            // SAFETY: `stale` 是本槽刚刚独占取出的堆指针（swap 保证只有一方拿到非空值）。
            drop(unsafe { Box::from_raw(stale) });
        }
    }

    /// 取走当前帧；没有待取帧时返回 `None`。
    ///
    /// 这里需要 unsafe：把原子交换拿到的裸堆指针还原成 `Box`。
    #[allow(unsafe_code)]
    pub(crate) fn take(&self) -> Option<T> {
        let raw = self.current.swap(ptr::null_mut(), Ordering::AcqRel);
        if raw.is_null() {
            None
        } else {
            // SAFETY: `raw` 由本线程独占取出：原子 swap 保证生产/消费只有一方拿到非空指针。
            Some(unsafe { *Box::from_raw(raw) })
        }
    }
}

impl<T: Send> Drop for FrameSlot<T> {
    /// 这里需要 unsafe：释放槽里可能残留的堆指针。
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let raw = *self.current.get_mut();
        if !raw.is_null() {
            // SAFETY: `&mut self` 保证没有并发访问，堆指针由本槽独占。
            drop(unsafe { Box::from_raw(raw) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slot_yields_none() {
        let slot = FrameSlot::<u32>::new();
        assert!(slot.take().is_none());
    }

    #[test]
    fn publish_then_take_returns_latest() {
        let slot = FrameSlot::new();
        slot.publish(1u32);
        slot.publish(2u32);
        assert_eq!(slot.take(), Some(2));
        assert_eq!(slot.take(), None);
    }
}
