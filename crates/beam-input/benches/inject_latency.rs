//! I-13 性能预算的验证基准：注入热路径「入队 → 注入完成 ≤1 ms」（§3.8 / §6）。
//!
//! ## 测的到底是什么（口径）
//!
//! | 基准 | 覆盖的段 | 说明 |
//! |---|---|---|
//! | `input/enqueue_only` | 生产者侧 | 只测 `submit()`：入队（有界队列 push + 时间戳） |
//! | `input/enqueue_to_dispatch` | 全链路 | 入队 → 队列等待 → 线程唤醒 → 注入线程执行 `apply` → 主线程收到回执 |
//!
//! **这个基准不会真的注入按键**：`SendInput` 有真实副作用（会真的按下按键 /
//! 移动鼠标），所以基准注入的是一个**只记账的假注入器**
//! （`InputInjector::with_sink`）。因此基准测的是
//! 「**构造 + 入队 + 出队 + 调度**」这一段的成本，
//! **不包含**真实 `SendInput` 的耗时（那部分必须在真机上人工验证）。
//!
//! 注入器内部统计的延迟口径是「**入队 → 注入完成**」（入队时打 `Instant`
//! 时间戳，`apply` 返回后取差值），见 `beam_input::InjectorStats`。
//! criterion 那一路的墙钟时间还多含「回执唤醒主线程」的一段，
//! 因此**内部统计值 ≤ criterion 值**，两个都打印出来。
//!
//! ## 预算判定
//!
//! 跑完会打印一行 `BUDGET CHECK`，给出 p50 / p99 / 最大延迟与 §3.8 的 1 ms 预算
//! 的对比。设置环境变量 `BEAM_BENCH_STRICT=1` 时，一旦 p99 超过预算即以非零
//! 退出码结束（用于 CI 门禁；默认只报告不失败）。
//!
//! ## 实测结果
//!
//! 由 `cargo bench -p beam-input --bench inject_latency` 产出
//! （本机：Windows x64 / rustc 1.86 / `bench` profile）。
//!
//! | 基准 | 实测区间（criterion time） |
//! |---|---|
//! | `input/enqueue_only` | 129.67 – 131.76 ns（均值 130.70 ns） |
//! | `input/enqueue_to_dispatch` | 501.61 – 515.90 ns（均值 508.88 ns） |
//!
//! 注入器内部记录的延迟口径（「入队 → 注入完成」，2000 个样本）：
//!
//! | 指标 | 实测 | §3.8 预算 |
//! |---|---|---|
//! | 均值 | 0.3 µs | ≤1 ms |
//! | p50 | 0.2 µs | ≤1 ms |
//! | p99 | 0.4 µs | ≤1 ms |
//! | 最大 | 88.5 µs | ≤1 ms |
//!
//! 直方图：`≤50µs:1999 ≤100µs:1 ≤250µs:0 ≤500µs:0 ≤1ms:0 >1ms:0` ——
//! **没有一次**超出 1 ms 预算。
//!
//! 注入线程优先级：**MMCSS "Games"**（本机 MMCSS 服务可用）。
//!
//! ### 复测时的抖动
//!
//! 同一台机器上连续三次运行（中间改过 MMCSS 注册逻辑），观测到：
//!
//! | 运行 | `enqueue_only` | `enqueue_to_dispatch` | 内部 p99 |
//! |---|---|---|---|
//! | 1 | 165.47 – 187.01 ns | 493.39 – 565.63 ns | 9.8 µs |
//! | 2 | 161.61 – 165.35 ns | 414.32 – 433.74 ns | 0.4 µs |
//! | 3（定稿） | 129.67 – 131.76 ns | 501.61 – 515.90 ns | 0.4 µs |
//!
//! 本机是共享环境，单次数百纳秒级的抖动正常；**最差的一次 p99 也只有预算的 1%**。
//! 结论：§3.8「接收 → 注入 ≤1 ms」在「入队 → 注入完成」这段上有**两个数量级余量**。
//! **不要为了迎合预算而改动注入实现**：预算不达标时如实记录新数字即可。

use std::hint::black_box;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use beam_input::{InputError, InputInjector, InputSink};
use beam_proto::{InputEvent, KeyAction};
use criterion::{criterion_group, Criterion};

/// §3.8 的预算：接收 → 注入 ≤1 ms。
const BUDGET: Duration = Duration::from_millis(1);

/// 基准用的输入事件：一次键盘按下（HID 扫描码 0x04 = 'a'）。
fn sample_event() -> InputEvent {
    InputEvent::Key {
        code: 0x04,
        action: KeyAction::Down,
    }
}

/// 只记账的假注入器：**不会**真的调用 `SendInput`。
///
/// 每处理一个事件就把计数器加一并唤醒等待者（当作「注入完成」的回执）。
struct AckSink {
    counter: Arc<(Mutex<u64>, Condvar)>,
}

impl AckSink {
    fn new(counter: Arc<(Mutex<u64>, Condvar)>) -> Self {
        Self { counter }
    }
}

impl InputSink for AckSink {
    fn apply(&mut self, _ev: InputEvent) -> Result<(), InputError> {
        let (lock, cv) = &*self.counter;
        let mut count = lock.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        cv.notify_all();
        Ok(())
    }
}

/// 等到假注入器处理了 `target` 个事件。
fn wait_for(counter: &Arc<(Mutex<u64>, Condvar)>, target: u64) {
    let (lock, cv) = &**counter;
    let mut count = lock.lock().unwrap_or_else(|e| e.into_inner());
    while *count < target {
        count = cv.wait(count).unwrap_or_else(|e| e.into_inner());
    }
}

/// 生产者侧：只测入队。
fn bench_enqueue_only(c: &mut Criterion) {
    let counter = Arc::new((Mutex::new(0u64), Condvar::new()));
    let injector = InputInjector::with_sink(Box::new(AckSink::new(Arc::clone(&counter))), 256);
    let event = sample_event();
    c.bench_function("input/enqueue_only", |b| {
        b.iter(|| {
            injector
                .submit(black_box(event.clone()))
                .expect("入队应当成功");
        });
    });
}

/// 全链路：入队 → 出队 → 线程唤醒 → 注入线程执行 → 回执。
fn bench_enqueue_to_dispatch(c: &mut Criterion) {
    let counter = Arc::new((Mutex::new(0u64), Condvar::new()));
    let injector = InputInjector::with_sink(Box::new(AckSink::new(Arc::clone(&counter))), 256);
    let event = sample_event();
    let mut done = 0u64;
    c.bench_function("input/enqueue_to_dispatch", |b| {
        b.iter(|| {
            done += 1;
            injector
                .submit(black_box(event.clone()))
                .expect("入队应当成功");
            wait_for(&counter, done);
        });
    });
}

criterion_group!(benches, bench_enqueue_only, bench_enqueue_to_dispatch);

/// 预算检查：跑一批完整的「入队 → 注入完成」，统计 p50 / p99 / 最大延迟。
///
/// 这里用的是**注入器内部记录的延迟**（入队 → `apply` 返回），
/// 也就是 §3.8「接收 → 注入 ≤1 ms」的那条口径（不含网络接收）。
fn budget_check() -> bool {
    /// 样本数。
    const SAMPLES: u32 = 2_000;

    let counter = Arc::new((Mutex::new(0u64), Condvar::new()));
    let injector = InputInjector::with_sink(Box::new(AckSink::new(Arc::clone(&counter))), 256);
    let event = sample_event();

    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLES as usize);
    for done in 1..=SAMPLES {
        injector.submit(event.clone()).expect("入队应当成功");
        wait_for(&counter, u64::from(done));
        if let Some(d) = injector.last_latency() {
            latencies.push(d);
        }
    }
    latencies.sort_unstable();

    let pick = |q: f64| -> Duration {
        if latencies.is_empty() {
            return Duration::ZERO;
        }
        let idx = ((latencies.len() as f64 - 1.0) * q).round() as usize;
        latencies[idx]
    };
    let p50 = pick(0.50);
    let p99 = pick(0.99);
    let max = latencies.last().copied().unwrap_or(Duration::ZERO);
    let mean = injector.stats().mean_latency().unwrap_or(Duration::ZERO);

    let ok = p99 <= BUDGET;
    println!(
        "BUDGET CHECK: 入队→注入完成 样本={} 均值={:.1}µs p50={:.1}µs p99={:.1}µs 最大={:.1}µs 预算={:.0}µs -> {}",
        latencies.len(),
        mean.as_secs_f64() * 1e6,
        p50.as_secs_f64() * 1e6,
        p99.as_secs_f64() * 1e6,
        max.as_secs_f64() * 1e6,
        BUDGET.as_secs_f64() * 1e6,
        if ok { "PASS" } else { "FAIL" },
    );
    println!("  注入线程优先级: {}", injector.priority_mode().as_str());
    match injector.stats().priority_error() {
        Some(code) => println!(
            "  MMCSS 注册失败,Win32 错误码={code}（1062 = ERROR_SERVICE_NOT_ACTIVE：MMSystem 服务没装 / 没跑）"
        ),
        None => println!("  MMCSS 注册:没有报错"),
    }
    println!("  统计摘要: {}", injector.snapshot().summary());
    ok
}

fn main() {
    benches();
    let ok = budget_check();
    // 默认只报告；CI 门禁可用 BEAM_BENCH_STRICT=1 让超标直接失败。
    if !ok && std::env::var_os("BEAM_BENCH_STRICT").is_some() {
        std::process::exit(1);
    }
}
