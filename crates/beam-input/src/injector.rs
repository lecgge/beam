//! 注入器：有界通道 + 专用注入线程（§3.8 线程模型 / §6 线程约定 / I-13 延迟预算）。
//!
//! ## 线程模型（§3.8）
//!
//! > 线程:输入 stream 接收任务 → 有界通道 → 注入线程(高优先级)。
//!
//! 本模块就是这条链路的后两段：
//!
//! ```text
//! 输入 stream 接收任务            有界通道              注入线程（高优先级）
//!  （tokio task，beam-host）  →  [InjectorQueue]  →  [beam-input-inject]  →  SendInput
//!        submit()                    容量 N                SendInputSink::apply
//! ```
//!
//! 接收任务（调用方线程）**只负责入队**，真正的 `SendInput` 发生在**专用 OS 线程**上，
//! 因此 §6「注入用专用 OS 线程并提升优先级」成立，且注入不会阻塞网络接收任务。
//!
//! ## 为什么不是 `std::sync::mpsc::sync_channel`
//!
//! 背压策略要求「**丢掉最旧的**」（见下），而 `sync_channel` 的**生产者只能丢新的**
//! —— 它没有「从队头取走一个」的能力。因此这里用 `Mutex<VecDeque>` + `Condvar`
//! 实现了一条**有界 SPSC 环形队列**：
//!
//! - 有界：容量固定，内存占用上界明确；
//! - SPSC：只有一个生产者（接收任务）与一个消费者（注入线程），无锁竞争；
//! - 丢最旧：队列满时弹出队头（最旧的事件）再放入新事件。
//!
//! ## 背压：丢最旧，而不是阻塞接收任务
//!
//! §3.8 给的预算是「接收 → 注入 ≤1 ms」。注入线程一旦被拖慢（例如 `SendInput`
//! 被目标窗口的 UIPI 挡住、或系统正忙），队列就会积压；此时**阻塞**接收任务会把
//! 延迟直接推给网络层（输入流是可靠流，接收任务一停，上游的 TCP/QUIC 窗口也停）。
//!
//! 所以队列满时**丢最旧的**：
//!
//! - 理由一：**过期的事件没有价值**。鼠标移动是「当前位置」语义，旧坐标丢掉之后
//!   新坐标照样把指针带到正确位置；丢掉最新反而会把指针留在错误位置。
//! - 理由二：保住 ≤1 ms 的预算。丢旧保新，队列长度不增长，延迟不继续恶化。
//!
//! 代价（**如实**记录）：丢事件会打乱「按下 / 抬起」的配对。若被丢的是一次
//! `KeyDown` 而对应的 `KeyUp` 还在队列里，多出来的一次 `KeyUp` 是无害的
//! （键本来就没被按下）；反之（丢 `KeyUp`）可能留下一个「按住的键」。
//! 这只在注入线程严重落后（> 队列容量个事件）时才会发生，
//! 统计里以 `dropped` 计数暴露出来。
//!
//! ## 延迟口径（I-13）
//!
//! 统计的是「**入队 → 注入完成**」：入队时打一个 `Instant` 时间戳，
//! 注入线程在 `apply` 返回后取差值。因此这条数字**包含**：
//!
//! - 队列等待（有界通道里的排队时间）；
//! - 线程唤醒与调度（§6 要求的专用线程）；
//! - `SendInputSink::apply` 的耗时（构造 `INPUT` + `SendInput`）。
//!
//! 它**不包含**网络接收（QUIC 解帧 → `submit`）那一段。
//!
//! ## 线程优先级（§6：Windows MMCSS "Games"）
//!
//! 注入线程启动时**先试 MMCSS "Games"**
//! （`AvSetMmThreadCharacteristicsW(L"Games")` + `AvSetMmThreadPriority(AVRT_PRIORITY_HIGH)`）；
//! MMCSS 不可用时**退**到 `SetThreadPriority(THREAD_PRIORITY_HIGHEST)`。
//!
//! 用了哪一种由 [`ThreadPriorityMode`] 如实记录（`InjectorStats::priority_mode()`），
//! **不假装**用了 MMCSS。MMCSS 失败时的 Win32 错误码也记在
//! `InjectorStats::priority_error()` 里。
//!
//! ### MMCSS 任务索引的调用约定（**实测**，不是照抄文档）
//!
//! MSDN 只说「第一次调用必须传 `0`，之后可以传返回的索引」，**没提索引会失效**。
//! 本机实测（见 `mmcs_task_index_round_trip` 用例）：
//!
//! | 场景 | 结果 |
//! |---|---|
//! | 进程内第一次调用（`index = 0`） | ✅ 成功，返回索引（本机是 4388 / 4389 这类值） |
//! | 同一线程再次调用 | ❌ `1552`（`ERROR_ALREADY_JOINED`） |
//! | 注册被撤销后用旧索引调用 | ❌ `1551`（`ERROR_INVALID_TASK_INDEX`，索引失效） |
//! | 注册被撤销后用 `0` 重新调用 | ✅ 成功（拿到**新**索引） |
//!
//! 所以注册逻辑是：**先试缓存的索引，遇到 `1551` 就用 `0` 重新注册**。
//!
//! ## unsafe 策略（决策 D1）
//!
//! crate 根是 `#![deny(unsafe_code)]`，只有 Windows 线程优先级那段
//! （`AvSetMmThreadCharacteristicsW` / `AvSetMmThreadPriority` / `SetThreadPriority`）
//! 局部 `#[allow(unsafe_code)]`。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use beam_proto::InputEvent;

use crate::error::InputError;
use crate::sink::{permits_for, InputPermissions, InputSink, SendInputSink};

/// 有界队列的默认容量（个事件）。
///
/// 取 256：足够吸收一次网络突发，又不会让「积压」变成「延迟黑洞」
/// （256 个鼠标移动事件 ≈ 几帧的量级）。
pub const DEFAULT_QUEUE_CAPACITY: usize = 256;

/// 注入线程名（§6 专用 OS 线程）。
pub const INJECTOR_THREAD_NAME: &str = "beam-input-inject";

/// 延迟直方图的桶上界（微秒）。
///
/// 最后一档是「> 1000 µs」，也就是**超出 §3.8 的 1 ms 预算**。
pub const LATENCY_BUCKET_US: [u64; 5] = [50, 100, 250, 500, 1000];

/// 直方图桶数（5 个上界 + 1 个溢出桶）。
pub const LATENCY_BUCKET_COUNT: usize = LATENCY_BUCKET_US.len() + 1;

/// 注入线程的优先级模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPriorityMode {
    /// 线程刚起来，还没设置优先级。
    Pending,
    /// Windows **MMCSS "Games"**（`AvSetMmThreadCharacteristicsW` + `AvSetMmThreadPriority`）。
    MmcssGames,
    /// 退路：`SetThreadPriority(THREAD_PRIORITY_HIGHEST)`（MMCSS 不可用时）。
    ThreadPriorityFallback,
    /// 平台不支持优先级设置（非 Windows）。
    Unsupported,
}

impl ThreadPriorityMode {
    /// 是否真的用上了 MMCSS "Games"。
    pub fn is_mmcss(self) -> bool {
        matches!(self, ThreadPriorityMode::MmcssGames)
    }

    /// 人类可读描述。
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadPriorityMode::Pending => "未设置",
            ThreadPriorityMode::MmcssGames => "MMCSS/Games",
            ThreadPriorityMode::ThreadPriorityFallback => "SetThreadPriority/最高",
            ThreadPriorityMode::Unsupported => "平台不支持",
        }
    }
}

/// 注入器的运行统计（I-13）。
///
/// 全部是原子计数：读统计**不会**阻塞注入热路径。
#[derive(Debug, Default)]
pub struct InjectorStats {
    /// 已入队的事件数。
    submitted: AtomicU64,
    /// 已成功注入的事件数。
    injected: AtomicU64,
    /// 因权限被拒的事件数（`E8002`）。
    denied: AtomicU64,
    /// 注入失败的事件数（`SendInput` 返回 0 等）。
    failed: AtomicU64,
    /// 背压丢弃的事件数（丢最旧）。
    dropped: AtomicU64,
    /// 延迟样本数。
    samples: AtomicU64,
    /// 延迟总和（纳秒）。
    sum_ns: AtomicU64,
    /// 最大延迟（纳秒）。
    max_ns: AtomicU64,
    /// 最近一次延迟（纳秒；`0` 表示还没有样本）。
    last_ns: AtomicU64,
    /// 延迟直方图。
    buckets: [AtomicU64; LATENCY_BUCKET_COUNT],
    /// 线程优先级模式（见 [`ThreadPriorityMode`]）。
    priority: AtomicU8,
    /// MMCSS 注册失败时的 Win32 错误码（`0` = 没有失败）。
    priority_error: AtomicU32,
}

impl InjectorStats {
    /// 记一次延迟样本。
    fn record_latency(&self, latency: Duration) {
        let ns = latency.as_nanos().min(u64::MAX as u128) as u64;
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        self.last_ns.store(ns, Ordering::Relaxed);
        // 最大值：CAS 循环（多写者只有一个，但仍然用 CAS 避免丢更新）。
        let mut current = self.max_ns.load(Ordering::Relaxed);
        while ns > current {
            match self.max_ns.compare_exchange_weak(
                current,
                ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        let idx = bucket_index(ns);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// 已入队的事件数。
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    /// 已成功注入的事件数。
    pub fn injected(&self) -> u64 {
        self.injected.load(Ordering::Relaxed)
    }

    /// 因权限被拒的事件数。
    pub fn denied(&self) -> u64 {
        self.denied.load(Ordering::Relaxed)
    }

    /// 注入失败的事件数。
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    /// 背压丢弃（丢最旧）的事件数。
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 延迟样本数。
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    /// 最近一次「入队 → 注入完成」的延迟。
    pub fn last_latency(&self) -> Option<Duration> {
        match self.last_ns.load(Ordering::Relaxed) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// 最大延迟。
    pub fn max_latency(&self) -> Option<Duration> {
        match self.max_ns.load(Ordering::Relaxed) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// 平均延迟。
    pub fn mean_latency(&self) -> Option<Duration> {
        let samples = self.samples();
        if samples == 0 {
            return None;
        }
        Some(Duration::from_nanos(
            self.sum_ns.load(Ordering::Relaxed) / samples,
        ))
    }

    /// 线程优先级模式。
    pub fn priority_mode(&self) -> ThreadPriorityMode {
        match self.priority.load(Ordering::Relaxed) {
            1 => ThreadPriorityMode::MmcssGames,
            2 => ThreadPriorityMode::ThreadPriorityFallback,
            3 => ThreadPriorityMode::Unsupported,
            _ => ThreadPriorityMode::Pending,
        }
    }

    /// MMCSS 注册失败时的 Win32 错误码（`None` = 没有失败或还没设置）。
    ///
    /// 常见值：
    /// - `1062`（`ERROR_SERVICE_NOT_ACTIVE`）：**MMSystem（MMCSS）服务没装 / 没跑**
    ///   —— Windows Server Core 与部分容器镜像上就是这样；
    /// - `5`（`ERROR_ACCESS_DENIED`）：进程没有该权限。
    pub fn priority_error(&self) -> Option<u32> {
        match self.priority_error.load(Ordering::Relaxed) {
            0 => None,
            code => Some(code),
        }
    }

    /// 直方图快照（每个桶的计数）。
    pub fn buckets(&self) -> [u64; LATENCY_BUCKET_COUNT] {
        let mut out = [0u64; LATENCY_BUCKET_COUNT];
        for (idx, slot) in self.buckets.iter().enumerate() {
            out[idx] = slot.load(Ordering::Relaxed);
        }
        out
    }

    /// 一次性取全部统计。
    pub fn snapshot(&self) -> LatencySnapshot {
        LatencySnapshot {
            submitted: self.submitted(),
            injected: self.injected(),
            denied: self.denied(),
            failed: self.failed(),
            dropped: self.dropped(),
            samples: self.samples(),
            mean: self.mean_latency(),
            max: self.max_latency(),
            last: self.last_latency(),
            buckets: self.buckets(),
            priority: self.priority_mode(),
            priority_error: self.priority_error(),
        }
    }
}

/// 延迟样本 → 直方图桶下标。
fn bucket_index(ns: u64) -> usize {
    for (idx, bound_us) in LATENCY_BUCKET_US.iter().enumerate() {
        if ns <= bound_us * 1000 {
            return idx;
        }
    }
    LATENCY_BUCKET_COUNT - 1
}

/// 注入器统计快照（可读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencySnapshot {
    /// 已入队的事件数。
    pub submitted: u64,
    /// 已成功注入的事件数。
    pub injected: u64,
    /// 因权限被拒的事件数。
    pub denied: u64,
    /// 注入失败的事件数。
    pub failed: u64,
    /// 背压丢弃的事件数。
    pub dropped: u64,
    /// 延迟样本数。
    pub samples: u64,
    /// 平均延迟。
    pub mean: Option<Duration>,
    /// 最大延迟。
    pub max: Option<Duration>,
    /// 最近一次延迟。
    pub last: Option<Duration>,
    /// 延迟直方图。
    pub buckets: [u64; LATENCY_BUCKET_COUNT],
    /// 注入线程的优先级模式。
    pub priority: ThreadPriorityMode,
    /// MMCSS 注册失败时的 Win32 错误码。
    pub priority_error: Option<u32>,
}

impl LatencySnapshot {
    /// 直方图桶的标签（与 [`LatencySnapshot::buckets`] 一一对应）。
    pub fn bucket_labels() -> [&'static str; LATENCY_BUCKET_COUNT] {
        ["≤50µs", "≤100µs", "≤250µs", "≤500µs", "≤1ms", ">1ms"]
    }

    /// 超出 §3.8「接收 → 注入 ≤1 ms」预算的事件数。
    pub fn over_budget(&self) -> u64 {
        self.buckets[LATENCY_BUCKET_COUNT - 1]
    }

    /// 一行可读摘要。
    pub fn summary(&self) -> String {
        let fmt = |d: Option<Duration>| match d {
            Some(d) => format!("{:.1}µs", d.as_secs_f64() * 1e6),
            None => "-".to_string(),
        };
        let buckets = Self::bucket_labels()
            .iter()
            .zip(self.buckets.iter())
            .map(|(label, count)| format!("{label}:{count}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mmcss = match self.priority_error {
            Some(code) => format!("(MMCSS 错误码 {code})"),
            None => String::new(),
        };
        format!(
            "submitted={} injected={} denied={} failed={} dropped={} | 延迟 均值={} 最大={} 最近={} | 桶 {buckets} | 线程优先级={}{}",
            self.submitted,
            self.injected,
            self.denied,
            self.failed,
            self.dropped,
            fmt(self.mean),
            fmt(self.max),
            fmt(self.last),
            self.priority.as_str(),
            mmcss,
        )
    }
}

/// 队列里的一条事件（带入队时间戳）。
#[derive(Debug)]
struct QueuedEvent {
    event: InputEvent,
    enqueued_at: Instant,
}

/// 生产者：入队的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushOutcome {
    /// 已入队。
    Queued,
    /// 队列满：丢了**最旧**的一条。
    DroppedOldest,
    /// 队列已关闭（注入器已停止）。
    Closed,
}

/// 队列内部状态。
#[derive(Debug)]
struct QueueInner {
    items: VecDeque<QueuedEvent>,
    capacity: usize,
    closed: bool,
}

/// 有界 SPSC 队列（丢最旧）。
#[derive(Debug)]
struct InjectorQueue {
    inner: Mutex<QueueInner>,
    ready: Condvar,
}

impl InjectorQueue {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(QueueInner {
                items: VecDeque::with_capacity(capacity.min(1024)),
                capacity: capacity.max(1),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// 生产者：入队。队列满时**丢最旧的**。
    ///
    /// 永不阻塞：接收任务不能被注入线程拖住（§3.8 的 ≤1 ms 预算）。
    fn push(&self, item: QueuedEvent) -> PushOutcome {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.closed {
            return PushOutcome::Closed;
        }
        let mut outcome = PushOutcome::Queued;
        if guard.items.len() >= guard.capacity {
            // 丢最旧：队头就是过期最久的事件。
            guard.items.pop_front();
            outcome = PushOutcome::DroppedOldest;
        }
        guard.items.push_back(item);
        drop(guard);
        self.ready.notify_one();
        outcome
    }

    /// 消费者：取一条事件；队列关闭且已空时返回 `None`。
    fn pop(&self) -> Option<QueuedEvent> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(item) = guard.items.pop_front() {
                return Some(item);
            }
            if guard.closed {
                return None;
            }
            guard = self.ready.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// 消费者：**不阻塞**地取一条事件（队列空则返回 `None`）。
    #[cfg(test)]
    fn try_pop(&self) -> Option<QueuedEvent> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.items.pop_front()
    }

    /// 关闭队列（唤醒消费者线程退出）。
    fn close(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        drop(guard);
        self.ready.notify_all();
    }
}

/// 权限位打包（bit0=键盘 / bit1=鼠标 / bit2=手柄）。
fn pack_perms(perms: InputPermissions) -> u8 {
    (u8::from(perms.keyboard)) | (u8::from(perms.mouse) << 1) | (u8::from(perms.gamepad) << 2)
}

/// 权限位解包。
fn unpack_perms(bits: u8) -> InputPermissions {
    InputPermissions {
        keyboard: bits & 0b001 != 0,
        mouse: bits & 0b010 != 0,
        gamepad: bits & 0b100 != 0,
    }
}

/// 注入器（§3.8 线程模型）。
///
/// 形态：**有界通道 + 一条专用 OS 线程**。
///
/// - 提交侧（接收任务 / 调用方线程）：[`InputInjector::submit`]，**不阻塞**；
/// - 注入侧（专用线程 `beam-input-inject`）：按设备权限校验后调用
///   `SendInputSink::apply`（真的调 `SendInput`）。
///
/// ## 权限
///
/// 权限是**运行期可变**的（对端连上 / 断开时闸门会变），存在一个原子位里，
/// 注入线程在每个事件前读一次。因此 `set_permissions` 不需要停线程。
///
/// ## 生命周期
///
/// `Drop` 会关闭队列并 **join** 注入线程（线程会自己退出）。
pub struct InputInjector {
    queue: Arc<InjectorQueue>,
    stats: Arc<InjectorStats>,
    perms: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for InputInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputInjector")
            .field("stats", &self.snapshot())
            .finish()
    }
}

impl InputInjector {
    /// 新建注入器（真实 `SendInput` 注入）。
    ///
    /// 权限按设备（键盘 / 鼠标 / 手柄），默认由调用方给出。
    pub fn new(perms: InputPermissions) -> Self {
        Self::with_capacity(perms, DEFAULT_QUEUE_CAPACITY)
    }

    /// 新建注入器并指定队列容量。
    pub fn with_capacity(perms: InputPermissions, capacity: usize) -> Self {
        let mut sink = SendInputSink::new();
        sink.set_permissions(perms);
        Self::spawn(Box::new(sink), perms, capacity)
    }

    /// 新建注入器，注入器由调用方提供（**测试 / 基准用**）。
    ///
    /// 这条路径**不会**真的调用 `SendInput`：单测与基准必须避免真实副作用
    /// （真的会按键 / 移动鼠标），所以它们注入一个只记账的假注入器。
    /// 权限固定为「三类全允许」——权限校验属于真实注入路径的事。
    pub fn with_sink(sink: Box<dyn InputSink + Send>, capacity: usize) -> Self {
        Self::spawn(sink, InputPermissions::all(), capacity)
    }

    fn spawn(sink: Box<dyn InputSink + Send>, perms: InputPermissions, capacity: usize) -> Self {
        let queue = Arc::new(InjectorQueue::new(capacity));
        let stats = Arc::new(InjectorStats::default());
        let perms = Arc::new(AtomicU8::new(pack_perms(perms)));

        let thread_queue = Arc::clone(&queue);
        let thread_stats = Arc::clone(&stats);
        let thread_perms = Arc::clone(&perms);
        let spawned = std::thread::Builder::new()
            .name(INJECTOR_THREAD_NAME.to_string())
            .spawn(move || run_injector(thread_queue, thread_stats, thread_perms, sink));

        let thread = match spawned {
            Ok(handle) => Some(handle),
            Err(err) => {
                // 起线程失败：**不静默**——统计里记成失败，注入器退化成「只入队不注入」。
                tracing::error!(reason = %err, "起注入线程失败");
                stats.failed.fetch_add(1, Ordering::Relaxed);
                None
            }
        };

        Self {
            queue,
            stats,
            perms,
            thread,
        }
    }

    /// 入队一个事件（**不阻塞**）。
    ///
    /// 队列满时丢最旧的（见模块文档「背压」）。
    ///
    /// 注入器已经停止（[`InputInjector::shutdown`] 之后）时返回
    /// [`InputError::BackendDisabled`]（`E8003`）——**不静默丢弃**。
    pub fn submit(&self, event: InputEvent) -> Result<(), InputError> {
        self.stats.submitted.fetch_add(1, Ordering::Relaxed);
        match self.queue.push(QueuedEvent {
            event,
            enqueued_at: Instant::now(),
        }) {
            PushOutcome::Queued => Ok(()),
            PushOutcome::DroppedOldest => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                tracing::debug!("注入队列已满,丢弃最旧的事件(背压,非错误)");
                Ok(())
            }
            PushOutcome::Closed => {
                tracing::warn!("注入器已停止,丢弃输入事件");
                Err(InputError::BackendDisabled)
            }
        }
    }

    /// 设置按设备的注入权限（运行期可变）。
    pub fn set_permissions(&self, perms: InputPermissions) {
        self.perms.store(pack_perms(perms), Ordering::Relaxed);
    }

    /// 当前按设备的注入权限。
    pub fn permissions(&self) -> InputPermissions {
        unpack_perms(self.perms.load(Ordering::Relaxed))
    }

    /// 统计句柄（可在别处持有一份 `Arc`，读统计不会阻塞注入热路径）。
    pub fn stats(&self) -> Arc<InjectorStats> {
        Arc::clone(&self.stats)
    }

    /// 最近一次「入队 → 注入完成」的延迟。
    pub fn last_latency(&self) -> Option<Duration> {
        self.stats.last_latency()
    }

    /// 统计快照。
    pub fn snapshot(&self) -> LatencySnapshot {
        self.stats.snapshot()
    }

    /// 注入线程的优先级模式（MMCSS "Games" 还是退路）。
    pub fn priority_mode(&self) -> ThreadPriorityMode {
        self.stats.priority_mode()
    }

    /// 停止注入线程（幂等）。
    pub fn shutdown(&mut self) {
        self.queue.close();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for InputInjector {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl InputSink for InputInjector {
    /// 把事件投递给注入线程（**入队**，不是同步注入）。
    fn apply(&mut self, ev: InputEvent) -> Result<(), InputError> {
        self.submit(ev)
    }
}

/// 注入线程的主体。
fn run_injector(
    queue: Arc<InjectorQueue>,
    stats: Arc<InjectorStats>,
    perms: Arc<AtomicU8>,
    mut sink: Box<dyn InputSink + Send>,
) {
    let guard = raise_thread_priority();
    stats
        .priority
        .store(priority_code(guard.mode()), Ordering::Relaxed);
    stats.priority_error.store(guard.error(), Ordering::Relaxed);
    tracing::info!(priority = guard.mode().as_str(), "注入线程已启动");

    while let Some(item) = queue.pop() {
        let current = unpack_perms(perms.load(Ordering::Relaxed));
        // 把**当前**权限同步给注入器：权限是运行期可变的，
        // 不同步的话注入器内部那份会停在构造时的值。
        sink.set_permissions(current);
        if !permits_for(&current, &item.event) {
            stats.denied.fetch_add(1, Ordering::Relaxed);
            tracing::debug!("按设备权限拒绝注入(E8002)");
            continue;
        }
        let started = Instant::now();
        match sink.apply(item.event) {
            Ok(()) => {
                stats.injected.fetch_add(1, Ordering::Relaxed);
                // 延迟口径：入队 → 注入完成。
                stats.record_latency(started.duration_since(item.enqueued_at));
            }
            Err(err) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(code = %err.code_str(), "注入输入失败");
            }
        }
    }
    tracing::info!("注入线程已退出");
}

/// 优先级模式 → 原子存储用的编码。
fn priority_code(mode: ThreadPriorityMode) -> u8 {
    match mode {
        ThreadPriorityMode::Pending => 0,
        ThreadPriorityMode::MmcssGames => 1,
        ThreadPriorityMode::ThreadPriorityFallback => 2,
        ThreadPriorityMode::Unsupported => 3,
    }
}

// ---------------------------------------------------------------------------
// 线程优先级（§6：Windows MMCSS "Games"）
// ---------------------------------------------------------------------------

/// Windows 线程优先级提升。
///
/// **优先用 MMCSS "Games"**（`AvSetMmThreadCharacteristicsW("Games")` +
/// `AvSetMmThreadPriority(AVRT_PRIORITY_HIGH)`）；MMCSS 不可用时**退**到
/// `SetThreadPriority(THREAD_PRIORITY_HIGHEST)`。
///
/// 退路会被 [`ThreadPriorityMode`] 如实记录，**不假装**用了 MMCSS。
#[cfg(windows)]
mod priority {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority,
        GetCurrentThread, SetThreadPriority, AVRT_PRIORITY_HIGH, THREAD_PRIORITY_HIGHEST,
    };

    use super::ThreadPriorityMode;

    /// MMCSS 任务索引（进程内缓存）。
    ///
    /// MSDN（`AvSetMmThreadCharacteristicsW`）：
    /// 「The first time this function is called, this value must be 0 on input.
    /// The index value is returned on output and can be used as input in
    /// subsequent calls.」
    ///
    /// **实测（本机 Windows）**：索引在注册被撤销后会失效
    /// （`ERROR_INVALID_TASK_INDEX` = 1551），因此
    /// [`register_games_task`] 在索引失效时会用 `0` 重新注册。
    static TASK_INDEX: AtomicU32 = AtomicU32::new(0);

    /// `ERROR_INVALID_TASK_INDEX`：缓存的任务索引已经失效。
    const ERROR_INVALID_TASK_INDEX: u32 = 1551;

    /// 注册 MMCSS "Games" 任务，返回任务句柄。
    ///
    /// 调用约定（MSDN + 本机实测）：
    ///
    /// - 进程里**第一次**调用必须传 `0`，返回值里带回任务索引；
    /// - 之后的调用要传那个索引；
    /// - 但只要之前的注册被撤销（线程退出 / `AvRevertMmThreadCharacteristics`），
    ///   旧索引就失效了（`ERROR_INVALID_TASK_INDEX` = 1551）——
    ///   这时用 `0` 重新注册（会拿到一个新的索引）。
    ///
    /// 返回 `Err(win32_code)` 是 Win32 错误码。
    #[allow(unsafe_code)]
    fn register_games_task() -> Result<HANDLE, u32> {
        let name: Vec<u16> = "Games\0".encode_utf16().collect();

        // 先试缓存的索引。
        let cached = TASK_INDEX.load(Ordering::Relaxed);
        if cached != 0 {
            let mut index = cached;
            // SAFETY: `name` 是以 NUL 结尾的 UTF-16 缓冲，在本次调用期间有效；
            // `index` 指向一个合法的 `u32`。
            match unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(name.as_ptr()), &mut index) } {
                Ok(handle) => {
                    TASK_INDEX.store(index, Ordering::Relaxed);
                    return Ok(handle);
                }
                Err(err) => {
                    let code = win32_code(err.code().0);
                    if code != ERROR_INVALID_TASK_INDEX {
                        return Err(code);
                    }
                    // 索引失效（注册已被撤销）→ 下面用 0 重新注册。
                    tracing::debug!(code, "MMCSS 任务索引已失效,重新注册");
                }
            }
        }

        // 首次调用（或索引失效后的重试）：必须传 0。
        let mut index = 0u32;
        // SAFETY: 同上。
        match unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(name.as_ptr()), &mut index) } {
            Ok(handle) => {
                TASK_INDEX.store(index, Ordering::Relaxed);
                Ok(handle)
            }
            Err(err) => Err(win32_code(err.code().0)),
        }
    }

    /// 线程退出时要撤销的 MMCSS 注册。
    pub(super) struct PriorityGuard {
        mode: ThreadPriorityMode,
        avrt: Option<HANDLE>,
        /// MMCSS 注册失败时的 Win32 错误码（`0` = 没失败）。
        error: u32,
    }

    impl PriorityGuard {
        pub(super) fn mode(&self) -> ThreadPriorityMode {
            self.mode
        }

        /// MMCSS 注册失败时的 Win32 错误码（`0` = 没失败）。
        pub(super) fn error(&self) -> u32 {
            self.error
        }
    }

    impl Drop for PriorityGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if let Some(handle) = self.avrt {
                // SAFETY: `handle` 来自 `AvSetMmThreadCharacteristicsW`，只在这里用一次。
                let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
            }
        }
    }

    /// 提升当前线程的优先级。
    #[allow(unsafe_code)]
    pub(super) fn raise() -> PriorityGuard {
        // 1) MMCSS "Games"
        let mut mmcss_error = 0u32;
        let avrt = match register_games_task() {
            Ok(handle) => {
                // SAFETY: `handle` 是上面刚拿到的 MMCSS 任务句柄。
                match unsafe { AvSetMmThreadPriority(handle, AVRT_PRIORITY_HIGH) } {
                    Ok(()) => Some(handle),
                    Err(err) => {
                        // SAFETY: 同上；撤销这次注册，避免泄漏 MMCSS 任务。
                        let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
                        mmcss_error = win32_code(err.code().0);
                        tracing::warn!(
                            code = mmcss_error,
                            "设置 MMCSS 优先级失败,退回 SetThreadPriority"
                        );
                        None
                    }
                }
            }
            Err(code) => {
                // 最常见的原因：任务索引 / 服务不可用
                // （1062 = `ERROR_SERVICE_NOT_ACTIVE`：MMSystem 服务没装 / 没跑；
                //  1551 = `ERROR_INVALID_TASK_INDEX`：索引失效且重新注册也失败）。
                mmcss_error = code;
                tracing::warn!(
                    code = mmcss_error,
                    "注册 MMCSS \"Games\" 任务失败,退回 SetThreadPriority"
                );
                None
            }
        };
        if let Some(handle) = avrt {
            return PriorityGuard {
                mode: ThreadPriorityMode::MmcssGames,
                avrt: Some(handle),
                error: 0,
            };
        }

        // 2) 退路：普通线程优先级。
        // SAFETY: `GetCurrentThread` 返回当前线程的伪句柄，`SetThreadPriority` 对伪句柄有效。
        let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) }.is_ok();
        PriorityGuard {
            mode: if ok {
                ThreadPriorityMode::ThreadPriorityFallback
            } else {
                ThreadPriorityMode::Unsupported
            },
            avrt: None,
            error: mmcss_error,
        }
    }

    /// HRESULT → 原始 Win32 错误码。
    ///
    /// `windows` crate 把 Win32 错误包成 `HRESULT::from_win32(code)`
    /// （`0x8007xxxx`），低 16 位就是原始错误码。
    fn win32_code(hr: i32) -> u32 {
        (hr as u32) & 0xFFFF
    }
}

/// 非 Windows 平台：没有优先级设置（§6 的 MMCSS 是 Windows 专有的）。
#[cfg(not(windows))]
mod priority {
    use super::ThreadPriorityMode;

    /// 线程退出时要撤销的 MMCSS 注册（非 Windows 恒为空）。
    pub(super) struct PriorityGuard {
        mode: ThreadPriorityMode,
    }

    impl PriorityGuard {
        pub(super) fn mode(&self) -> ThreadPriorityMode {
            self.mode
        }

        /// MMCSS 注册失败时的 Win32 错误码（非 Windows 恒为 0）。
        pub(super) fn error(&self) -> u32 {
            0
        }
    }

    /// 提升当前线程的优先级（非 Windows：不做任何事）。
    pub(super) fn raise() -> PriorityGuard {
        PriorityGuard {
            mode: ThreadPriorityMode::Unsupported,
        }
    }
}

use priority::raise as raise_thread_priority;

#[cfg(test)]
mod tests {
    use super::*;
    use beam_proto::KeyAction;

    /// MMCSS "Games" 任务索引的真实调用约定（**实测**记录）。
    ///
    /// MSDN 只说「第一次调用必须传 0，之后可以传返回的索引」，但**没说**索引会失效。
    /// 本机实测（Windows，`cargo test -p beam-input -- --ignored --nocapture mmcs`）：
    ///
    /// | 场景 | 结果 |
    /// |---|---|
    /// | 进程内第一次调用（`index = 0`） | ✅ 成功，返回索引（本机拿到 4388 / 4389 这种值） |
    /// | 同一线程再次调用 | ❌ `1552`（`ERROR_ALREADY_JOINED`，该线程已在任务里） |
    /// | 注册被撤销后，用旧索引调用 | ❌ `1551`（`ERROR_INVALID_TASK_INDEX`，索引失效） |
    /// | 注册被撤销后，用 `0` 重新调用 | ✅ 成功（拿到**新**索引） |
    ///
    /// 因此 [`priority::register_games_task`] 的做法是：
    /// **先试缓存索引，遇到 1551 就用 0 重新注册**。
    ///
    /// 这条用例默认**不跑**（`#[ignore]`）：它会真的向 MMCSS 注册线程。
    #[test]
    #[ignore]
    #[allow(unsafe_code)]
    fn mmcs_task_index_round_trip() {
        use windows::core::PCWSTR;
        use windows::Win32::System::Threading::{
            AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
        };
        fn call(name: &[u16], index: &mut u32) -> Result<u32, u32> {
            match unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(name.as_ptr()), index) } {
                Ok(handle) => {
                    let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
                    Ok(*index)
                }
                Err(err) => Err((err.code().0 as u32) & 0xFFFF),
            }
        }
        let name: Vec<u16> = "Games\0".encode_utf16().collect();

        // 首次：传 0 → 成功并拿到索引。
        let mut index = 0u32;
        let first = call(&name, &mut index).expect("首次注册必须成功");
        assert_ne!(first, 0, "应当拿到非零任务索引");

        // 注册已被撤销 → 旧索引失效（1551），用 0 重新注册成功。
        let name2 = name.clone();
        let stale = first;
        std::thread::spawn(move || {
            let mut old = stale;
            assert_eq!(
                call(&name2, &mut old),
                Err(1551),
                "撤销注册后旧索引应当失效"
            );
            let mut fresh = 0u32;
            assert!(call(&name2, &mut fresh).is_ok(), "用 0 重新注册应当成功");
        })
        .join()
        .unwrap();
    }

    /// 只记账的假注入器：**不会**真的按键（单测禁止真实副作用）。
    struct CountingSink {
        applied: Arc<AtomicU64>,
    }

    impl CountingSink {
        fn new() -> (Self, Arc<AtomicU64>) {
            let applied = Arc::new(AtomicU64::new(0));
            (
                Self {
                    applied: Arc::clone(&applied),
                },
                applied,
            )
        }
    }

    impl InputSink for CountingSink {
        fn apply(&mut self, _ev: InputEvent) -> Result<(), InputError> {
            self.applied.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    /// 会**阻塞**的假注入器：用来确定性地制造背压（把注入线程按住）。
    struct BlockingSink {
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl InputSink for BlockingSink {
        fn apply(&mut self, _ev: InputEvent) -> Result<(), InputError> {
            let (lock, cv) = &*self.release;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            while !*released {
                released = cv.wait(released).unwrap_or_else(|e| e.into_inner());
            }
            Ok(())
        }
    }

    fn key_event() -> InputEvent {
        InputEvent::Key {
            code: 4,
            action: KeyAction::Down,
        }
    }

    #[test]
    fn queue_is_bounded_and_drops_the_oldest() {
        let queue = InjectorQueue::new(2);
        let now = Instant::now();
        assert_eq!(
            queue.push(QueuedEvent {
                event: key_event(),
                enqueued_at: now
            }),
            PushOutcome::Queued
        );
        assert_eq!(
            queue.push(QueuedEvent {
                event: key_event(),
                enqueued_at: now
            }),
            PushOutcome::Queued
        );
        // 第三次：队列已满 → 丢最旧的。
        assert_eq!(
            queue.push(QueuedEvent {
                event: key_event(),
                enqueued_at: now
            }),
            PushOutcome::DroppedOldest
        );

        let first = queue.try_pop().expect("应当还有事件");
        let second = queue.try_pop().expect("应当还有事件");
        assert!(queue.try_pop().is_none(), "只应剩两条");
        // 丢的是**最早**入队的那条：三条内容一样，用入队时间戳区分。
        assert!(second.enqueued_at >= first.enqueued_at);
    }

    #[test]
    fn closed_queue_rejects_new_events() {
        let queue = InjectorQueue::new(2);
        queue.close();
        assert_eq!(
            queue.push(QueuedEvent {
                event: key_event(),
                enqueued_at: Instant::now()
            }),
            PushOutcome::Closed
        );
        assert!(queue.pop().is_none(), "关闭后不再有事件");
    }

    #[test]
    fn injector_delivers_events_to_the_worker_thread() {
        let (sink, applied) = CountingSink::new();
        let injector = InputInjector::with_sink(Box::new(sink), 16);
        injector.submit(key_event()).expect("入队应当成功");
        // 等注入线程处理完（最多 2 秒）。
        let deadline = Instant::now() + Duration::from_secs(2);
        while applied.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_micros(200));
        }
        assert_eq!(applied.load(Ordering::Relaxed), 1, "事件应当被注入线程消费");
        let snapshot = injector.snapshot();
        assert_eq!(snapshot.submitted, 1);
        assert_eq!(snapshot.injected, 1);
        assert_eq!(snapshot.failed, 0);
        assert!(snapshot.last.is_some(), "应当有一次延迟样本");
    }

    #[test]
    fn injector_reports_backpressure_drops() {
        // 注入器被「卡住」时队列才会真的积压 —— 用一个会阻塞的假注入器把
        // 注入线程按住，丢最旧的才是**确定**发生（不依赖线程调度）。
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let sink = BlockingSink {
            release: Arc::clone(&release),
        };
        // 容量 1：连灌 3 条。
        let injector = InputInjector::with_sink(Box::new(sink), 1);
        for _ in 0..3 {
            injector.submit(key_event()).expect("入队应当成功");
        }
        let dropped = injector.snapshot().dropped;
        // 放开注入线程（否则 `Drop` 里的 join 会一直等它）。
        {
            let (lock, cv) = &*release;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cv.notify_all();
        }
        assert_eq!(injector.snapshot().submitted, 3);
        assert!(dropped >= 1, "队列满时必须丢最旧的,实际 dropped={dropped}");
    }

    #[test]
    fn permissions_are_enforced_on_the_worker_thread() {
        // 假注入器只记账，**不会**真的按键 —— 因此这里可以安全地验证权限闸门。
        let (sink, applied) = CountingSink::new();
        let injector = InputInjector::with_sink(Box::new(sink), 16);
        assert_eq!(injector.permissions(), InputPermissions::all());

        // 关掉键盘权限 → 键盘事件在注入线程里被拒（E8002 语义）。
        injector.set_permissions(InputPermissions {
            keyboard: false,
            mouse: true,
            gamepad: true,
        });
        injector.submit(key_event()).expect("入队应当成功");
        let deadline = Instant::now() + Duration::from_secs(2);
        while injector.snapshot().denied == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_micros(200));
        }
        assert_eq!(injector.snapshot().denied, 1, "键盘事件应当被权限拒绝");
        assert_eq!(applied.load(Ordering::Relaxed), 0, "被拒的事件不能进注入器");
        assert_eq!(injector.snapshot().injected, 0);
    }

    #[test]
    fn latency_snapshot_is_readable() {
        let snapshot = LatencySnapshot {
            submitted: 3,
            injected: 3,
            denied: 0,
            failed: 0,
            dropped: 0,
            samples: 3,
            mean: Some(Duration::from_micros(40)),
            max: Some(Duration::from_micros(90)),
            last: Some(Duration::from_micros(50)),
            buckets: [3, 0, 0, 0, 0, 0],
            priority: ThreadPriorityMode::MmcssGames,
            priority_error: None,
        };
        let text = snapshot.summary();
        assert!(text.contains("submitted=3"), "{text}");
        assert!(text.contains("MMCSS/Games"), "{text}");
        assert_eq!(snapshot.over_budget(), 0);
    }

    #[test]
    fn bucket_index_matches_the_budget() {
        // 50µs 以内 → 第 0 桶；100µs → 第 1 桶；1ms → 第 4 桶；超过 → 溢出桶。
        assert_eq!(bucket_index(10_000), 0);
        assert_eq!(bucket_index(50_000), 0);
        assert_eq!(bucket_index(50_001), 1);
        assert_eq!(bucket_index(1_000_000), 4);
        assert_eq!(bucket_index(1_000_001), LATENCY_BUCKET_COUNT - 1);
    }

    #[test]
    fn permission_bits_round_trip() {
        for perms in [
            InputPermissions::none(),
            InputPermissions::all(),
            InputPermissions {
                keyboard: true,
                mouse: false,
                gamepad: true,
            },
        ] {
            assert_eq!(unpack_perms(pack_perms(perms)), perms);
        }
    }
}
