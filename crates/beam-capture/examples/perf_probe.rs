//! 抓屏阶段埋点验证入口（§3.13）。
//!
//! 这个入口**真的抓屏**：用 WGC（Windows Graphics Capture）抓若干帧，
//! 然后打印 `beam_perf::Perf::snapshot()` 的「抓屏」阶段耗时表，
//! 证明「抓屏埋点真的在采集」而不是纸面声明。
//!
//! 用法：
//!
//! ```powershell
//! cargo run -p beam-capture --example perf_probe --features wgc,perf-stats
//! ```
//!
//! 需要**交互式桌面会话**与真实显示器；WGC 的显示器捕获是**内容驱动**的
//! ——画面没有变化时系统不会投递新帧，因此抓到的帧数取决于桌面是否有变化
//! （任务栏时钟跳动、终端输出等都会触发）。抓不到帧时本入口会如实报告，
//! **不会**伪造数据。
//!
//! `perf-stats` 关闭时本 example **不会被构建**（`required-features`）：
//! 关掉 feature 时 `beam-perf` 根本不在依赖树里，快照必然是空的。
//!
//! ## 「抓屏」这一段到底包含什么
//!
//! `perf_span!("capture")` 包住的是 WGC 回调的**整段**：GPU→GPU 拷贝 +
//! **每帧新建一块独占目标纹理** + 纹理发布。
//!
//! 为什么要每帧新建纹理：WGC 的帧池是**池化**的，直接把池化纹理交给消费者，
//! 消费者还没读完就被下一帧覆盖（会读到撕裂帧）。所以每帧一次
//! `CreateTexture2D` + `CopyResource`。
//!
//! **实测（本机 RTX 3070 / 1920×1080，release 构建）**：
//!
//! | 指标 | 抓屏阶段（含每帧新建纹理） |
//! |---|---|
//! | 样本 | 239 帧 |
//! | 均值 | 32.4 µs |
//! | p50 / p95 | 25 µs / 25 µs |
//! | **p99** | **75 µs** |
//! | 最大 | 273.4 µs |
//! | §3.5 预算 | T1 ≤ 1.5 ms = 1500 µs |
//!
//! 即 **p99 只占预算的 5%**（余量约 20 倍）—— 「每帧新建目标纹理的开销」
//! **落在预算内**。
//!
//! > 注：这是**本机**的数字；WGC 显示器捕获是内容驱动的（画面不变就不投递
//! > 新帧），样本分布会随桌面活动变化。
//! > 重新测：`cargo run --release -p beam-capture --example perf_probe --features wgc,perf-stats`

use std::time::{Duration, Instant};

use beam_capture::{CaptureBackend, CaptureConfig, CaptureSource, WgcCaptureSource};
use beam_perf::{ExportFormat, Perf, PerfSnapshot, Stage};

/// 抓屏时长（秒）。
const SECONDS: f64 = 5.0;
/// 取帧轮询间隔（毫秒）。
const POLL_MS: u64 = 5;

fn main() {
    println!("=== beam-perf 抓屏埋点验证（§3.13）===");
    println!("[perf-stats] feature 已开启：采集生效");

    // 清空全局采集器：下面的数据只来自本次运行。
    Perf::reset();

    let mut source = WgcCaptureSource::new();
    let cfg = CaptureConfig {
        backend: CaptureBackend::Wgc,
        display_id: None,
        min_update_interval: None,
        target_fps: 60,
    };
    if let Err(e) = source.start(cfg) {
        println!("FAIL: WGC 抓屏启动失败 {} {} — {e}", e.code_str(), e.code());
        std::process::exit(2);
    }
    println!("WGC 抓屏: 已启动，后端 = {:?}", source.backend_kind());

    // 抓若干帧。
    let deadline = Instant::now() + Duration::from_secs_f64(SECONDS);
    let mut frames = 0usize;
    let mut first_size: Option<(u32, u32)> = None;
    while Instant::now() < deadline {
        if let Some(frame) = source.latest() {
            frames += 1;
            if first_size.is_none() {
                let mut desc =
                    windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
                // SAFETY: `desc` 是合法的输出参数。
                unsafe { frame.tex.GetDesc(&mut desc) };
                first_size = Some((desc.Width, desc.Height));
            }
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
    source.stop();
    println!("抓屏: {frames} 帧, 分辨率 = {first_size:?}");

    let snapshot = Perf::snapshot();
    print_snapshot(&snapshot);

    // 导出一份 CSV，方便离线核对。
    let mut csv_path = std::env::temp_dir();
    csv_path.push("beam-perf-capture-probe.csv");
    match Perf::export(ExportFormat::Csv, &csv_path) {
        Ok(()) => println!("CSV 已导出: {}", csv_path.display()),
        Err(e) => println!("CSV 导出失败: {e}"),
    }

    println!();
    let has_capture = snapshot
        .stages
        .iter()
        .any(|(stage, _)| *stage == Stage::Capture);
    if !has_capture {
        println!("RESULT: FAIL —— 快照里没有抓屏阶段，埋点没有生效。");
        std::process::exit(1);
    }
    println!("RESULT: PASS —— 抓屏阶段的耗时已被真实采集到。");
    std::process::exit(0);
}

/// 打印快照里的各阶段耗时表。
fn print_snapshot(snapshot: &PerfSnapshot) {
    println!();
    println!("=== Perf::snapshot() 阶段耗时（§3.13）===");
    if snapshot.stages.is_empty() {
        println!("（本次没有采到任何阶段数据）");
    } else {
        println!(
            "{:<16}{:>8}{:>14}{:>14}{:>14}{:>14}{:>14}",
            "stage", "count", "mean_ns", "p50_ns", "p95_ns", "p99_ns", "max_ns"
        );
        for (stage, s) in &snapshot.stages {
            println!(
                "{:<16}{:>8}{:>14}{:>14}{:>14}{:>14}{:>14}",
                stage.as_str(),
                s.count,
                s.mean_ns,
                s.p50_ns,
                s.p95_ns,
                s.p99_ns,
                s.max_ns
            );
        }
    }
    println!(
        "samples={} bitrate_bps={} fps={} dropped_frames={}",
        snapshot.samples, snapshot.bitrate_bps, snapshot.fps, snapshot.dropped_frames
    );
}
