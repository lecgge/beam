//! 真机「解码纹理 → 原生窗口上屏」验证（§3.10 / 产品方案第 45 行）。
//!
//! ## 为什么需要这个入口
//!
//! §3.10 规定「解码纹理直接交给 D3D11 flip-model 交换链」，但这条链路
//! 在真正跑起来之前**没有任何证据**：
//!
//! - `beam-decode/examples/d3d11va_decode_smoke.rs` 只证明「硬解能出纹理」；
//! - `beam-render` 侧的窗口与交换链没有独立验证入口。
//!
//! 本入口把三段**接在一起**跑一遍：
//!
//! 1. 建**独立原生窗口**（`beam_render::VideoWindow`，不走 WebView）；
//! 2. 在窗口上建 D3D11 flip-model 交换链，拿到渲染层的 D3D11 设备；
//! 3. 用**同一个设备**打开 D3D11VA 硬解（§3.10：D3D11 不允许跨设备拷贝），
//!    逐帧解码并 `present` 到交换链；
//! 4. 读回交换链的 `GetFrameStatistics`，确认画面**真的上屏了**
//!    （`PresentCount` 递增），而不是「提交了但没画出来」。
//!
//! ## 用法
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! # 码流由编码侧产出：
//! # cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg
//! cargo run -p beam-client --example present_smoke --features video --release
//! ```
//!
//! 窗口会**真的弹出来**（本机有交互式桌面时），停留几秒后自动关闭。

use std::time::Duration;

use beam_decode::{D3D11VaDecoder, DecodeConfig, VideoDecoder};
use beam_proto::{AssembledFrame, CodecKind};
use beam_render::{
    D3D11Renderer, RenderConfig, Renderer, SwapChainFormat, VideoWindow, WindowHandle,
};
use bytes::Bytes;

/// 码流文件（由 beam-encode 的验证入口写出）。
fn stream_path() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push("beam-nvenc-smoke.bin");
    p
}

/// 解析「长度前缀的访问单元」序列（与编码侧写出的格式一致）。
fn parse_units(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut units = Vec::new();
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
    units
}

fn main() {
    println!("=== beam-client 真机「解码 → 原生窗口上屏」验证 ===");

    let path = stream_path();
    let raw = match std::fs::read(&path) {
        Ok(r) => r,
        Err(e) => {
            println!("FAIL: 读不到码流文件 {}: {e}", path.display());
            println!("提示：先跑 cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg");
            std::process::exit(2);
        }
    };
    let units = parse_units(&raw);
    println!("码流: {} 字节, {} 个访问单元", raw.len(), units.len());
    if units.is_empty() {
        println!("FAIL: 码流里没有访问单元");
        std::process::exit(1);
    }

    // ---- [1] 独立原生窗口（§3.10：画面不走 WebView）----
    let window = match VideoWindow::create("Beam 上屏验证", 1920, 1080) {
        Ok(w) => w,
        Err(e) => {
            println!("FAIL: 创建原生窗口失败：{e}");
            std::process::exit(2);
        }
    };
    println!("原生窗口: 创建成功（hwnd = 0x{:x}）", window.hwnd());

    // ---- [2] 先建一个 BGRA 交换链，拿到渲染层的 D3D11 设备 ----
    //
    // 设备只是用来「借用」给硬解上下文（D3D11 不允许跨设备拷贝），
    // 真正的交换链在对照实验里按选定方案重建。
    let render_cfg = RenderConfig {
        width: 1920,
        height: 1080,
        refresh_rate: 60,
        ..RenderConfig::default()
    };
    let mut renderer = D3D11Renderer::new();
    if let Err(e) = renderer.attach(WindowHandle(window.hwnd()), render_cfg.clone()) {
        println!("FAIL: 交换链创建失败 {} — {e}", e.code_str());
        std::process::exit(1);
    }
    println!("初始交换链: 创建成功（flip-model，BGRA 1920x1080）");
    if !renderer.attached() {
        println!("FAIL: 渲染器没有绑定窗口");
        std::process::exit(1);
    }

    // ---- [3] 用**同一个设备**打开硬解 ----
    let Some(device) = renderer.device() else {
        println!("FAIL: 渲染层没有 D3D11 设备");
        std::process::exit(1);
    };
    let decode_cfg = DecodeConfig {
        codec: CodecKind::Hevc,
        hardware: true,
        low_latency: true,
    };
    let mut decoder = match D3D11VaDecoder::open_with_device(decode_cfg.clone(), device) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: 打开 D3D11VA 解码器失败 {} — {e}", e.code_str());
            std::process::exit(1);
        }
    };
    println!("D3D11VA 解码器: 建立成功（复用渲染层同一个设备）");

    // ---- [4] 解码第一帧，拿到解码纹理的真实格式与尺寸 ----
    let first = units.first().expect("码流非空");
    let frame = AssembledFrame {
        frame_id: 0,
        pts_us: 0,
        capture_ts_us: 0,
        is_key: true,
        payload: Bytes::from(first.clone()),
    };
    let decoded_first = match decoder.decode(frame) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: 第一帧解码失败 {} — {e}", e.code_str());
            std::process::exit(1);
        }
    };
    println!(
        "解码纹理: {}x{}（可见高度由解码器报告）",
        decoded_first.width, decoded_first.height
    );

    // ---- [5] 对照实验：哪种交换链格式 × 尺寸能真的把纹理搬上屏 ----
    //
    // D3D11VA 视频解码的原生输出是 `DXGI_FORMAT_420Opaque`，且纹理高度
    // 是**对齐后**的 coded height（可能大于可见高度）。零拷贝要求交换链
    // 与它**格式相同、尺寸相同、同设备**。
    //
    // 这里把候选组合逐个真机试一遍，用 `PresentInfo` 判定成败 ——
    // 不靠「调用没报错」这种会骗人的判据。
    println!();
    println!("--- 交换链方案对照实验 ---");
    let formats = [
        SwapChainFormat::Opaque420,
        SwapChainFormat::Nv12,
        SwapChainFormat::Bgra8,
    ];
    // 候选高度：可见高度、对齐到 16 / 32 / 128。
    let heights = [
        decoded_first.height,
        align_up(decoded_first.height, 16),
        align_up(decoded_first.height, 32),
        align_up(decoded_first.height, 128),
    ];
    let mut winner: Option<(SwapChainFormat, u32)> = None;
    for fmt in formats {
        for h in heights {
            // 每个组合用**独立的窗口**：DXGI 规定一个 HWND 只能挂一个交换链，
            // 在同一个窗口上反复 attach 必然失败（那是探针的问题，不是方案的问题）。
            let probe_window =
                match VideoWindow::create("Beam 上屏方案探针", decoded_first.width, h) {
                    Ok(w) => w,
                    Err(e) => {
                        println!(
                            "  {:<28} {}x{}: 窗口创建失败 —— {e}",
                            fmt.name(),
                            decoded_first.width,
                            h
                        );
                        continue;
                    }
                };
            let cfg = RenderConfig {
                width: decoded_first.width,
                height: h,
                format: fmt,
                ..RenderConfig::default()
            };
            let mut r = D3D11Renderer::new();
            if let Err(e) = r.attach(WindowHandle(probe_window.hwnd()), cfg) {
                println!(
                    "  {:<28} {}x{}: 交换链创建失败 —— {e}",
                    fmt.name(),
                    decoded_first.width,
                    h
                );
                continue;
            }
            // 用刚解出的那一帧试一次真实呈现。
            let ok = r.present(&decoded_first).is_ok();
            let info = r.present_info();
            let verdict = if info.textured_presents > 0 {
                "零拷贝成功"
            } else if ok {
                "没搬上纹理（降级）"
            } else {
                "没搬上纹理（已如实报错 E10015）"
            };
            println!(
                "  {:<28} {}x{}: {verdict}（真搬 {} 次 / 降级 {} 次，源格式 0x{:08x}）",
                fmt.name(),
                decoded_first.width,
                h,
                info.textured_presents,
                info.cleared_presents,
                info.source_format,
            );
            if ok && info.textured_presents > 0 && winner.is_none() {
                winner = Some((fmt, h));
            }
        }
    }

    let Some((win_fmt, win_height)) = winner else {
        println!();
        println!("RESULT: FAIL —— 没有任何一个交换链方案能把解码纹理零拷贝搬上屏");
        println!(
            "  原因: D3D11VA 视频解码的原生输出是 DXGI_FORMAT_420Opaque (4:2:0 双平面),\n\
             而 flip-model 交换链不接受 4:2:0 视频格式; BGRA 交换链又与它格式不同。\n\
             => 零拷贝不可行, 需要先做 4:2:0 -> BGRA 的格式转换 (PSV + 像素着色器)。\n\
             => 详见 crates/beam-render 的 SwapChainFormat / PresentInfo 文档。"
        );
        std::process::exit(1);
    };
    println!();
    println!(
        "选定方案: {} @ {}x{}（零拷贝搬纹理）",
        win_fmt.name(),
        decoded_first.width,
        win_height
    );

    // ---- [6] 用选定方案把整段码流真的上屏 ----
    let cfg = RenderConfig {
        width: decoded_first.width,
        height: win_height,
        format: win_fmt,
        ..RenderConfig::default()
    };
    let mut renderer = D3D11Renderer::new();
    if let Err(e) = renderer.attach(WindowHandle(window.hwnd()), cfg) {
        println!("FAIL: 交换链创建失败 {e}");
        std::process::exit(1);
    }
    // 解码器要复用**同一个**设备（D3D11 不允许跨设备拷贝）。
    let mut decoder =
        match D3D11VaDecoder::open_with_device(decode_cfg.clone(), renderer.device().unwrap()) {
            Ok(d) => d,
            Err(e) => {
                println!("FAIL: 打开 D3D11VA 解码器失败 {} — {e}", e.code_str());
                std::process::exit(1);
            }
        };

    let mut decoded = 0usize;
    let mut presented = 0usize;
    for (i, unit) in units.iter().enumerate() {
        let frame = AssembledFrame {
            frame_id: i as u32,
            pts_us: i as u64 * 16_666,
            capture_ts_us: i as u64 * 16_666,
            is_key: i == 0,
            payload: Bytes::from(unit.clone()),
        };
        let out = match decoder.decode(frame) {
            Ok(o) => o,
            Err(e) => {
                println!("第 {i} 帧解码失败 {} — {e}", e.code_str());
                continue;
            }
        };
        decoded += 1;
        if renderer.present(&out).is_ok() {
            presented += 1;
        }
    }

    let info = renderer.present_info();
    println!(
        "解码输出: {decoded} 帧；上屏调用成功: {presented} 帧；渲染器自计 presented = {}",
        renderer.presented()
    );
    println!(
        "呈现诊断: 总呈现 {} 次，其中真的搬纹理 {} 次，降级清屏 {} 次",
        info.presents, info.textured_presents, info.cleared_presents
    );
    println!(
        "格式: 解码纹理 = 0x{:08x} ({}x{})，交换链 = 0x{:08x}，一致? {}",
        info.source_format,
        info.source_width,
        info.source_height,
        info.swapchain_format,
        if info.formats_match() { "是" } else { "否" }
    );

    // 停留一会儿，让用户真的看见窗口（也是给交换链一次 vsync 的机会）。
    std::thread::sleep(Duration::from_secs(2));

    if decoded == 0 {
        println!("RESULT: FAIL —— 没有解出任何帧");
        std::process::exit(1);
    }
    if presented == 0 {
        println!("RESULT: FAIL —— 没有任何一帧上屏成功");
        std::process::exit(1);
    }
    if info.textured_presents == 0 {
        println!(
            "RESULT: FAIL —— {} 次呈现**全部**降级，解码纹理没搬进交换链",
            info.presents
        );
        std::process::exit(1);
    }
    println!(
        "RESULT: PASS —— 解码纹理真的通过 flip-model 交换链上屏了（{} 次真实搬纹理）",
        info.textured_presents
    );
    // 显式退出：跳过第三方 DLL 在 main 返回后的 atexit 清理。
    std::process::exit(0);
}

/// 向上对齐到 `align` 的倍数。
fn align_up(value: u32, align: u32) -> u32 {
    value.div_ceil(align) * align
}
