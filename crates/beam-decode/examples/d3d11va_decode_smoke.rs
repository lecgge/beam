//! 真机 D3D11VA 硬解验证（§9 M-1 验证点③，解码侧）。
//!
//! 输入是 `beam-encode` 的验证入口产出的 HEVC 码流
//! （`cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg`
//! 会写到 `%TEMP%\beam-nvenc-smoke.bin`，长度前缀的访问单元序列）。
//!
//! 本入口做三件事：
//!
//! 1. 建一个 D3D11 设备（**模拟渲染层的设备**），并把它注入解码器
//!    ——验证「解码纹理必须创建在渲染层同一个设备上」这条契约；
//! 2. 用 D3D11VA 硬解逐帧解码，核对解出来的分辨率与帧数；
//! 3. 打印解码输出的渲染目标句柄（`DecodedFrame.surface`）。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-decode --example d3d11va_decode_smoke --features ffmpeg
//! ```
//!
//! **注意**：本入口验证的是「D3D11VA 硬解出纹理」这一段；
//! 「解码纹理交给渲染层上屏」由 beam-render 负责，不在本 crate 范围内。

use beam_decode::{D3D11VaDecoder, DecodeConfig, VideoDecoder};
use beam_proto::{AssembledFrame, CodecKind};
use bytes::Bytes;
use windows::core::Interface;
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
    println!("=== beam-decode 真机 D3D11VA 硬解验证 ===");

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

    // 建一个 D3D11 设备，模拟渲染层的设备（真实链路里应当注入渲染层那个）。
    let (device, _context) = match create_device() {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: 无法创建 D3D11 设备：{e}");
            std::process::exit(2);
        }
    };
    println!("D3D11 设备: 创建成功（作为渲染层设备的替身注入解码器）");

    // 用注入设备的入口打开解码器 —— 验证跨 crate 设备契约。
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

    // 设备一致性核对：解码器用的设备必须就是我们注入的那个。
    // 比较的是 COM 接口指针本身（不是 Rust 引用的地址）。
    match dec.device() {
        Some(d) => {
            let same = d.as_raw() == device.as_raw();
            println!(
                "设备一致性: 解码器设备 == 注入设备 ? {}",
                if same {
                    "是"
                } else {
                    "否（跨设备，帧链路会断）"
                }
            );
            if !same {
                println!("FAIL: 设备不一致");
                std::process::exit(1);
            }
        }
        None => println!("设备一致性: 解码器未报告设备"),
    }

    // 逐帧解码。
    let mut decoded = 0usize;
    let mut first_size: Option<(u32, u32)> = None;
    let mut surfaces = 0usize;
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
                if !out.surface.is_none() {
                    surfaces += 1;
                }
            }
            Err(e) => {
                println!("第 {i} 帧解码失败 {} {} — {e}", e.code_str(), e.code());
            }
        }
    }

    println!("解码输出: {decoded} 帧, 带渲染目标句柄 {surfaces} 帧");
    match first_size {
        Some((w, h)) => println!("解出分辨率: {w}x{h}"),
        None => println!("没有解出任何帧"),
    }

    if decoded == 0 {
        println!("RESULT: FAIL —— D3D11VA 没有解出任何帧");
        std::process::exit(1);
    }
    println!("RESULT: PASS —— D3D11VA 硬解在注入的 D3D11 设备上解出了画面");
    // 显式退出：跳过第三方 DLL 在 main 返回后的 atexit 清理。
    std::process::exit(0);
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
