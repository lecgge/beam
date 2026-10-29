//! 采集开销基准（§3.13「采集开销 < 1% CPU，单点 < 200 ns」）。
//!
//! 用法：
//!
//! ```text
//! cargo bench -p beam-perf                    # 默认（perf-stats 关闭）
//! cargo bench -p beam-perf --features perf-stats   # 真实采集路径
//! ```
//!
//! 默认构建里 `perf-stats` 关闭，`Perf::record` 是空实现，测出来的
//! 「单点耗时」没有意义；因此**真实数字必须看带 `--features perf-stats`
//! 的那次运行**。两种构建都测的部分（直方图记录、环形缓冲写入）是
//! `collect::record` 的实际内层开销，任何构建下都有参考价值。
//!
//! 自行实现计时循环而不用 criterion 等第三方基准框架：本仓库要求
//! `cargo tree -e features --workspace` 里 `perf-stats` 命中 0，
//! 引入新依赖会牵动整个工作区的依赖树与许可检查，收益不匹配。

use std::hint::black_box;
use std::time::{Duration, Instant};

use beam_perf::{Histogram, RingBuffer, Sample, Stage};

/// 每轮迭代次数。
const ITERS: u64 = 200_000;
/// 重复轮次（取最小值，规避调度抖动）。
const ROUNDS: u32 = 7;
/// 预热迭代次数。
const WARMUP: u64 = 20_000;

/// 一次测量的结果（纳秒 / 次）。
#[derive(Debug, Clone, Copy)]
struct Measurement {
    /// 最小值（纳秒 / 次）。
    min_ns: f64,
    /// 中位数（纳秒 / 次）。
    median_ns: f64,
    /// 最大值（纳秒 / 次）。
    max_ns: f64,
}

/// 跑 `iters` 次 `f`，返回每次调用的耗时统计（纳秒）。
fn measure<F: FnMut(u64)>(mut f: F) -> Measurement {
    for i in 0..WARMUP {
        f(i);
    }
    let mut samples = Vec::with_capacity(ROUNDS as usize);
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for i in 0..ITERS {
            f(i);
        }
        let elapsed = start.elapsed();
        samples.push(elapsed.as_nanos() as f64 / ITERS as f64);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("NaN 不应出现"));
    Measurement {
        min_ns: samples[0],
        median_ns: samples[samples.len() / 2],
        max_ns: samples[samples.len() - 1],
    }
}

fn report(name: &str, m: Measurement) {
    println!(
        "{name:<34} min {:>8.1} ns | median {:>8.1} ns | max {:>8.1} ns",
        m.min_ns, m.median_ns, m.max_ns
    );
}

/// 由单点耗时推导「1% CPU」预算下的可承受采样率。
///
/// 1% CPU = 每秒 10 ms = 10_000_000 ns 的采集时间预算。
/// 单点耗时 `t` ns 时，每秒可承受 `10_000_000 / t` 个采样点。
fn samples_per_second_under_one_percent(t_ns: f64) -> f64 {
    if t_ns <= 0.0 {
        f64::INFINITY
    } else {
        10_000_000.0 / t_ns
    }
}

/// 典型串流负载下的采集点数：60 fps × 11 个阶段。
const TYPICAL_FPS: f64 = 60.0;
const TYPICAL_STAGES: f64 = 11.0;

fn print_budget(m: Measurement) {
    let per_second = samples_per_second_under_one_percent(m.median_ns);
    let typical_points = TYPICAL_FPS * TYPICAL_STAGES;
    let typical_cpu_percent = typical_points * m.median_ns / 10_000_000.0 * 100.0;
    println!("  → 1% CPU 预算下每秒可承受 {:>12.0} 个采样点", per_second);
    println!(
        "  → 典型负载 {typical_points:.0} 点/秒（{TYPICAL_FPS:.0} fps × {TYPICAL_STAGES:.0} 阶段）占 CPU {typical_cpu_percent:.4}%"
    );
}

fn main() {
    println!("=== beam-perf 采集开销基准 ===");
    println!(
        "perf-stats feature: {}",
        if cfg!(feature = "perf-stats") {
            "开启（测的是真实采集路径）"
        } else {
            "关闭（Perf::record 为空实现，下面的 record 数字不代表真实开销）"
        }
    );
    println!("迭代 {ITERS} 次 × {ROUNDS} 轮，取最小 / 中位 / 最大：\n");

    // ---- 1. 直方图单点记录（collect::record 的内层开销之一）----
    let mut hist = Histogram::new();
    let m_hist = measure(|i| {
        hist.record(black_box(Duration::from_nanos(1000 + i % 100)));
    });
    report("直方图 Histogram::record", m_hist);
    println!(
        "  → 1% CPU 预算下每秒可承受 {:>12.0} 个采样点",
        samples_per_second_under_one_percent(m_hist.median_ns)
    );

    // ---- 2. 环形缓冲单点写入（collect::record 的内层开销之二）----
    let mut ring = RingBuffer::default();
    let m_ring = measure(|i| {
        ring.push(Sample {
            ts_us: i,
            stage: Stage::Encode,
            value_ns: 1000,
        });
    });
    report("环形缓冲 RingBuffer::push", m_ring);
    println!(
        "  → 1% CPU 预算下每秒可承受 {:>12.0} 个采样点",
        samples_per_second_under_one_percent(m_ring.median_ns)
    );

    // ---- 3. 公开 API 单点采集：Perf::record ----
    let m_record = measure(|i| {
        beam_perf::Perf::record(
            black_box(Stage::Encode),
            black_box(Duration::from_nanos(1000 + i % 100)),
        );
    });
    report("公开 API Perf::record（单点）", m_record);
    print_budget(m_record);

    // ---- 4. 批量吞吐：连续 N 次采集的总耗时 ----
    let batch = 10_000;
    let start = Instant::now();
    for i in 0..batch {
        beam_perf::Perf::record(
            black_box(Stage::Encode),
            black_box(Duration::from_nanos(1000 + i % 100)),
        );
    }
    let batch_elapsed = start.elapsed();
    println!(
        "\n批量吞吐：{batch} 次采集共 {:?} → {:.1} ns/次，{:.2} M 次/秒",
        batch_elapsed,
        batch_elapsed.as_nanos() as f64 / batch as f64,
        batch as f64 / batch_elapsed.as_secs_f64() / 1e6,
    );

    // ---- 5. 快照与导出开销（低频操作，不在热路径预算内）----
    let start = Instant::now();
    let snap = beam_perf::Perf::snapshot();
    let snap_elapsed = start.elapsed();
    println!(
        "快照 Perf::snapshot：{snap_elapsed:?}（采样点 {}，阶段 {}）",
        snap.samples,
        snap.stages.len()
    );

    println!("\n结论以 `--features perf-stats` 的运行为准。");
}
