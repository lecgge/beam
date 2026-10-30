//! 解码阶段埋点验证入口（§3.13）。
//!
//! 这个入口**真的解码**：喂进 `beam-encode` 的验证入口产出的真实 HEVC 码流
//! （`cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg`
//! 会写到 `%TEMP%\beam-nvenc-smoke.bin`，长度前缀的访问单元序列），
//! 用 D3D11VA 硬解逐帧解开，然后打印 `beam_perf::Perf::snapshot()` 的
//! 「解码」阶段耗时表，证明「解码埋点真的在采集」。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-decode --example perf_probe --features ffmpeg,perf-stats
//! ```
//!
//! `perf-stats` 关闭时本 example **不会被构建**（`required-features`）：
//! 关掉 feature 时 `beam-perf` 根本不在依赖树里，快照必然是空的。

use beam_decode::{D3D11VaDecoder, DecodeConfig, VideoDecoder};
use beam_perf::{ExportFormat, Perf, PerfSnapshot, Stage};
use beam_proto::{AssembledFrame, CodecKind};
use bytes::Bytes;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};

/// 码流文件（由 beam-encode 的验证入口写出）。
fn stream_path() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push("beam-nvenc-smoke.bin");
    p
}

fn main() {
    println!("=== beam-perf 解码埋点验证（§3.13）===");
    println!("[perf-stats] feature 已开启：采集生效");

    let path = stream_path();
    let raw = match std::fs::read(&path) {
        Ok(r) => r,
        Err(e) => {
            println!("FAIL: 读不到码流文件 {}: {e}", path.display());
            println!("提示：先跑 cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg");
            std::process::exit(2);
        }
    };

    // 解析「长度前缀的访问单元」序列。
    let mut units: Vec<Vec<u8>> = Vec::new();
    let mut off = 0usize;
    while off + 4 <= raw.len() {
        let len = u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]) as usize;
        off += 4;
        if off + len > raw.len() {
            break;
        }
        units.push(raw[off..off + len].to_vec());
        off += len;
    }
    println!("码流: {} 字节, {} 个访问单元", raw.len(), units.len());
    if units.is_empty() {
        println!("FAIL: 码流里没有访问单元");
        std::process::exit(1);
    }

    // 清空全局采集器：下面的数据只来自本次运行。
    Perf::reset();

    let (device, _context) = match create_device() {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: 无法创建 D3D11 设备：{e}");
            std::process::exit(2);
        }
    };
    println!("D3D11 设备: 创建成功（作为渲染层设备的替身注入解码器）");

    let cfg = DecodeConfig {
        codec: CodecKind::Hevc,
        hardware: true,
        low_latency: true,
    };
    let mut dec = match D3D11VaDecoder::open_with_device(cfg, &device) {
        Ok(d) => d,
        Err(e) => {
            println!(
                "FAIL: 打开 D3D11VA 解码器失败 {} {} — {e}",
                e.code_str(),
                e.code()
            );
            std::process::exit(1);
        }
    };
    println!("D3D11VA 解码器: 建立成功");

    // 逐帧解码。
    let mut decoded = 0usize;
    let mut first_size: Option<(u32, u32)> = None;
    for (i, unit) in units.iter().enumerate() {
        let frame = AssembledFrame {
            frame_id: i as u32,
            pts_us: i as u64 * 16_666,
            capture_ts_us: i as u64 * 16_666,
            is_key: i == 0,
            payload: Bytes::from(unit.clone()),
        };
        match dec.decode(frame) {
            Ok(out) => {
                decoded += 1;
                if first_size.is_none() {
                    first_size = Some((out.width, out.height));
                }
            }
            Err(e) => {
                println!("第 {i} 帧解码失败 {} {} — {e}", e.code_str(), e.code());
            }
        }
    }
    println!("解码输出: {decoded} 帧, 分辨率 = {first_size:?}");

    let snapshot = Perf::snapshot();
    print_snapshot(&snapshot);

    // 导出一份 CSV，方便离线核对。
    let mut csv_path = std::env::temp_dir();
    csv_path.push("beam-perf-decode-probe.csv");
    match Perf::export(ExportFormat::Csv, &csv_path) {
        Ok(()) => println!("CSV 已导出: {}", csv_path.display()),
        Err(e) => println!("CSV 导出失败: {e}"),
    }

    println!();
    let has_decode = snapshot
        .stages
        .iter()
        .any(|(stage, _)| *stage == Stage::Decode);
    if !has_decode {
        println!("RESULT: FAIL —— 快照里没有解码阶段，埋点没有生效。");
        std::process::exit(1);
    }
    println!("RESULT: PASS —— 解码阶段的耗时已被真实采集到。");
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

/// 创建 D3D11 设备。
fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
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
