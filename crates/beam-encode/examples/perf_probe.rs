//! 性能埋点验证入口（§3.13）。
//!
//! 这个入口**真的编码**：造一张 D3D11 NV12 纹理 → 送进 NVENC → 拿到真实码流，
//! 然后打印 `beam_perf::Perf::snapshot()` 的各阶段耗时表。
//! 目的：让「埋点真的在采集」这件事**可被肉眼验证**，而不是纸面声明。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"   # ffmpeg 的第三方运行库不在 conda 包的 bin 里
//! cargo run -p beam-encode --example perf_probe --features ffmpeg,perf-stats
//! ```
//!
//! `perf-stats` 关闭时本 example **不会被构建**（`required-features`）：
//! 关掉 feature 时 `beam-perf` 根本不在依赖树里，快照必然是空的，
//! 跑出来的表没有意义。
//!
//! ## 诚实性约定
//!
//! - 表里的每个数字都是**真实采样**得来的（`beam-perf` 的全局采集器）；
//! - CPU / GPU / 内存没有真实采样源时打印 `unavailable`，**不填 0 冒充**；
//! - 某个阶段没有数据就不出现在表里（而不是补一行 0）。

use std::cell::RefCell;
use std::time::Instant;

use beam_encode::{CapturedFrame, EncodeConfig, EncoderKind, HardwareVideoEncoder, VideoEncoder};
use beam_perf::{ExportFormat, Perf, PerfSnapshot, Stage};
use beam_proto::CodecKind;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_VIDEO_ENCODER, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};

// 持有 D3D11 设备直到进程结束。
//
// Rust 的 `static` **不会**在退出时被 drop —— 这正是这里需要的：
// NVIDIA 驱动侧的第三方 atexit 回调会在 C 运行时收尾阶段访问 D3D11 设备，
// 设备若已被释放，那个回调就访问了悬垂对象 → 0xC0000005。
// 详见 `main` 末尾的说明。
thread_local! {
    static KEEP_ALIVE: RefCell<Option<ID3D11Device>> = const { RefCell::new(None) };
}

/// 输入分辨率。
const WIDTH: u32 = 1920;
/// 输入分辨率。
const HEIGHT: u32 = 1080;
/// 编码帧数。
const FRAMES: u32 = 30;
/// 目标码率。
const BITRATE: u32 = 8_000_000;
/// 帧率。
const FPS: u32 = 60;

fn main() {
    println!("=== beam-perf 埋点验证（§3.13）===");
    println!("[perf-stats] feature 已开启：采集生效");

    let (device, _context) = match create_device() {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: 无法创建 D3D11 设备：{e}");
            std::process::exit(2);
        }
    };
    println!("D3D11 设备: 创建成功");

    // 清空全局采集器：下面的数据只来自本次运行。
    Perf::reset();

    let tex = match create_nv12_texture(&device) {
        Ok(t) => t,
        Err(e) => {
            println!("FAIL: 无法创建 NV12 纹理：{e}");
            std::process::exit(2);
        }
    };

    println!();
    println!("--- 真实编码 {FRAMES} 帧（NVENC / HEVC）---");
    let started = Instant::now();
    let (frames_out, bytes_out) = match run_encode(&tex) {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: 编码失败：{e}");
            std::process::exit(1);
        }
    };
    let elapsed = started.elapsed();

    // 码率 / 帧率按**实测**时长与实测字节数折算，不填预设值。
    let secs = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let measured_bps = (bytes_out as f64 * 8.0 / secs) as u32;
    let measured_fps = (frames_out as f64 / secs) as u32;
    Perf::set_counters(measured_bps, measured_fps, 0);
    println!(
        "[encode] 产出 {frames_out} 帧 / {bytes_out} 字节，用时 {:.3} ms，实测码率 {} bit/s，实测帧率 {}",
        elapsed.as_secs_f64() * 1000.0,
        measured_bps,
        measured_fps
    );

    let snapshot = Perf::snapshot();
    print_snapshot(&snapshot);

    // 导出一份 CSV，方便离线核对（导出路径由调用方决定）。
    let mut csv_path = std::env::temp_dir();
    csv_path.push("beam-perf-probe.csv");
    match Perf::export(ExportFormat::Csv, &csv_path) {
        Ok(()) => println!("CSV 已导出: {}", csv_path.display()),
        Err(e) => println!("CSV 导出失败: {e}"),
    }

    println!();
    let has_encode = snapshot
        .stages
        .iter()
        .any(|(stage, _)| *stage == Stage::Encode);
    if !has_encode {
        println!("RESULT: FAIL —— 快照里没有编码阶段，埋点没有生效。");
        std::process::exit(1);
    }
    println!("RESULT: PASS —— 编码阶段的耗时已被真实采集到。");
    println!("[teardown] 保留 D3D11 设备（不 drop）…");
    // **不要**在这里 `drop(device)`，也**不要**用 `std::process::exit(0)` 绕过。
    //
    // 背景：B3 记录过「main 返回后 NVIDIA 驱动侧的第三方 atexit 回调访问
    // 已释放对象 → 0xC0000005」，当时用 `std::process::exit(0)` 规避。
    //
    // **诚实说明**：本机（驱动 596.49）**已经复现不出**这个崩溃了 ——
    // 实测「先 drop 设备再让 main 正常返回」与「保持设备存活」两种写法
    // 都是 exit 0。所以下面这个改动**不是**「已验证的修复」，而是
    // **更稳妥的写法**：
    //   - 不用 `process::exit(0)` 绕过 CRT 收尾（真实应用退出走的就是
    //     「main 返回 → CRT 收尾」这条路，示例的绕过不代表库层面解决了）；
    //   - 让设备活到进程结束之后，atexit 回调访问到的是**有效**对象。
    //
    // 代价：进程结束时可能泄漏一块 D3D11 设备（进程一结束就归还给驱动）。
    KEEP_ALIVE.with(|slot| *slot.borrow_mut() = Some(device));
    println!("[teardown] 完成");
}

/// 跑一轮真实编码，返回（产出的帧数，码流总字节数）。
fn run_encode(tex: &ID3D11Texture2D) -> Result<(usize, usize), String> {
    let cfg = EncodeConfig {
        encoder: EncoderKind::Nvenc,
        codec: CodecKind::Hevc,
        bitrate_bps: BITRATE,
        fps: FPS,
        slices: 0,
        sfe: false,
        idr_interval: 0,
    };
    let mut enc = HardwareVideoEncoder::open(cfg).map_err(|e| format!("{e}"))?;
    println!("[encode] 编码器上下文: 建立成功");

    let mut frames = 0usize;
    let mut bytes = 0usize;
    let mut buffering = 0u32;
    for i in 0..FRAMES {
        let frame = CapturedFrame {
            tex: tex.clone(),
            ts_us: u64::from(i) * 1_000_000 / u64::from(FPS),
            dirty: true,
            width: WIDTH,
            height: HEIGHT,
        };
        match enc.encode(&frame) {
            Ok(encoded) => {
                frames += 1;
                bytes += encoded.to_payload().len();
            }
            // 硬件编码器有一帧延迟：首次调用还没有输出是正常状态。
            Err(e) if e.code() == 6005 => buffering += 1,
            Err(e) => {
                return Err(format!(
                    "第 {i} 帧编码失败 {} {} — {e}",
                    e.code_str(),
                    e.code()
                ))
            }
        }
    }
    if let Some(last) = enc.flush(u64::from(FRAMES) * 1_000_000 / u64::from(FPS)) {
        frames += 1;
        bytes += last.to_payload().len();
    }
    println!("[encode] 编码器缓冲中 {buffering} 次（硬件编码器的一帧延迟，正常状态）");
    Ok((frames, bytes))
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
    // CPU / GPU / 内存：没有真实采样源时是 None —— 如实打印 unavailable，不填 0。
    let memory = match (snapshot.memory_used_bytes(), snapshot.memory_total_bytes()) {
        (Some(used), Some(total)) => format!("{used}/{total}"),
        _ => "unavailable".to_string(),
    };
    println!(
        "system: cpu={} gpu={} memory={}",
        optional_percent(snapshot.cpu_percent()),
        optional_percent(snapshot.gpu_percent()),
        memory
    );
}

/// 可选百分比 → 文本；没有采样数据时是 `unavailable`（不写 0）。
fn optional_percent(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.2}"),
        None => "unavailable".to_string(),
    }
}

/// 创建 D3D11 设备。
fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
            windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
            Some(&mut device as *mut Option<ID3D11Device>),
            None,
            Some(&mut context as *mut Option<ID3D11DeviceContext>),
        )
    }
    .map_err(|e| format!("{e:?}"))?;
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err("D3D11CreateDevice 返回空指针".to_string()),
    }
}

/// 创建一张 NV12 纹理（硬件帧路径的输入）。
fn create_nv12_texture(device: &ID3D11Device) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: WIDTH,
        Height: HEIGHT,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_VIDEO_ENCODER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    let res = unsafe {
        device.CreateTexture2D(
            &desc,
            None,
            Some(&mut texture as *mut Option<ID3D11Texture2D>),
        )
    };
    match res {
        Ok(()) => texture.ok_or_else(|| "CreateTexture2D 成功后必须返回纹理".to_string()),
        Err(e) => Err(format!("{e:?}")),
    }
}
