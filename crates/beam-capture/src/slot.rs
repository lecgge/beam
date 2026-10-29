//! 单槽帧交换（§3.5 线程模型 / §6 热路径约定）。
//!
//! WGC 的回调线程与编码节拍线程之间只保留「最新一帧」：
//! 生产者覆盖式发布，消费者取走当前帧，取不到就说明这一拍没有新帧。
//!
//! §6 明确要求「热路径禁止阻塞锁，使用单槽交换或有界 SPSC」，
//! 因此这里用原子指针实现**无锁单槽**：交换本身是原子的，
//! 生产者与消费者各自最多拿到一份所有权，不存在双份释放。

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// 无锁单槽：发布 / 取走「最新一帧」。
///
/// 只有 `wgc` feature（真实抓屏后端）才会用到它；
/// 默认构建下它只是闲置，因此用 `cfg_attr` 静音 dead_code（单元测试仍然跑）。
#[cfg_attr(not(feature = "wgc"), allow(dead_code))]
pub(crate) struct FrameSlot<T: Send> {
    /// 当前帧的堆指针；为空表示没有待取帧。
    current: AtomicPtr<T>,
}

impl<T: Send> FrameSlot<T> {
    /// 新建一个空槽。
    #[cfg_attr(not(feature = "wgc"), allow(dead_code))]
    pub(crate) const fn new() -> Self {
        Self {
            current: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// 发布最新一帧（覆盖旧帧）。
    ///
    /// 旧帧如果还没被取走，就在这里释放——这正是「单槽」语义：
    /// 慢消费者只会拿到最新的一帧，不会积压延迟。
    ///
    /// 这里需要 unsafe：无锁单槽用裸堆指针在生产/消费之间转移所有权。
    #[allow(unsafe_code)]
    #[cfg_attr(not(feature = "wgc"), allow(dead_code))]
    pub(crate) fn publish(&self, value: T) {
        let fresh = Box::into_raw(Box::new(value));
        // 原子交换：旧帧要么被本线程拿走，要么已经被消费者取走。
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
    #[cfg_attr(not(feature = "wgc"), allow(dead_code))]
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
    use std::sync::Arc;
    use std::thread;

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
        // 单槽：只保留最新的一帧。
        assert_eq!(slot.take(), Some(2));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn producer_and_consumer_share_the_slot() {
        let slot = Arc::new(FrameSlot::new());
        let producer = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            for i in 0..1_000u32 {
                producer.publish(i);
            }
        });
        let mut seen = 0u32;
        while seen < 1_000 {
            if let Some(v) = slot.take() {
                assert!(v < 1_000);
                seen = v + 1;
            }
        }
        handle.join().expect("生产线程不应 panic");
    }
}
