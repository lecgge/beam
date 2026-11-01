//! 真机诊断：D3D11 VideoProcessor 能不能把 4:2:0 解码纹理当输入（§3.10）。
//!
//! ## 背景
//!
//! B17 的对照实验证明：**零拷贝不可行** —— flip-model 交换链不接受
//! `DXGI_FORMAT_420Opaque` / `DXGI_FORMAT_NV12`，而 D3D11 的 Pixel Shader View
//! 在 windows 绑定里没有（`ID3D11PixelShaderView` / `D3D11_VIEW_DESC` 都不存在）。
//!
//! 剩下的正规途径是 **D3D11 VideoProcessor**（硬件做格式转换）。
//!
//! `beam-encode` 那边试过 VP 并**全灭**（96 组变体），但它的源纹理是**自造的**
//! NV12 / BGRA，FourCC 取的是 DXGI 枚举值或 `'W8B8'`。
//! 本探针的源纹理是**解码器真实输出**的 `DXGI_FORMAT_420Opaque`，
//! 其真 FOURCC 是 `'420O'`（`0x4F303234`）—— 那个值**从没被试过**。
//!
//! ## 判据
//!
//! 「哪个 FourCC 让输入视图建得起来」由 `beam-render::vp::probe_video_processor`
//! **真机实测**给出；本入口只负责把解码纹理喂进去并如实打印结果。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-client --example vp_probe --features video --release
//! ```

use beam_decode::{D3D11VaDecoder, DecodeConfig, VideoDecoder};
use beam_proto::{AssembledFrame, CodecKind};
use beam_render::D3D11Renderer;
use bytes::Bytes;

fn stream_path() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push("beam-nvenc-smoke.bin");
    p
}

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
    println!("=== D3D11 VideoProcessor 对 4:2:0 解码纹理的可用性诊断 ===");

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
    if units.is_empty() {
        println!("FAIL: 码流里没有访问单元");
        std::process::exit(1);
    }

    // 解码器要用渲染层的同一个设备（D3D11 不允许跨设备拷贝）。
    // 建一个窗口 + 交换链只为拿到设备；VP 诊断本身不需要窗口。
    let window = match beam_render::VideoWindow::create("Beam VP 诊断", 1920, 1080) {
        Ok(w) => w,
        Err(e) => {
            println!("FAIL: 创建原生窗口失败：{e}");
            std::process::exit(2);
        }
    };
    let mut renderer = D3D11Renderer::new();
    let render_cfg = beam_render::RenderConfig {
        width: 1920,
        height: 1080,
        ..beam_render::RenderConfig::default()
    };
    if let Err(e) = renderer.attach(beam_render::WindowHandle(window.hwnd()), render_cfg) {
        println!("FAIL: 交换链创建失败 {} — {e}", e.code_str());
        std::process::exit(1);
    }
    let decode_cfg = DecodeConfig {
        codec: CodecKind::Hevc,
        hardware: true,
        low_latency: true,
    };
    let mut decoder = match D3D11VaDecoder::open_with_device(
        decode_cfg,
        renderer.device().expect("渲染层应当有 D3D11 设备"),
    ) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: 打开 D3D11VA 解码器失败 {} — {e}", e.code_str());
            std::process::exit(1);
        }
    };

    let first = units.first().expect("码流非空");
    let frame = AssembledFrame {
        frame_id: 0,
        pts_us: 0,
        capture_ts_us: 0,
        is_key: true,
        payload: Bytes::from(first.clone()),
    };
    let decoded = match decoder.decode(frame) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: 第一帧解码失败 {} — {e}", e.code_str());
            std::process::exit(1);
        }
    };
    println!(
        "解码纹理: 可见 {}x{}，句柄 = 0x{:x}",
        decoded.width, decoded.height, decoded.surface.raw
    );

    let probe = match beam_render::vp::probe_video_processor(decoded.surface.raw) {
        Ok(p) => p,
        Err(e) => {
            println!("FAIL: 诊断失败：{e}");
            std::process::exit(1);
        }
    };

    println!(
        "源纹理: DXGI 格式 0x{:08x}，{}x{}（对齐后的 coded height）",
        probe.source_format, probe.source_width, probe.source_height
    );
    println!(
        "枚举器: {}",
        if probe.enumerator_ok {
            "建成"
        } else {
            "建不出来"
        }
    );
    if let Some(err) = &probe.enumerator_error {
        println!("  原因: {err}");
    }
    println!("FourCC 候选矩阵:");
    for (name, err) in &probe.fourcc_errors {
        println!("  {name}: 失败 — {err}");
    }
    match probe.working_fourcc {
        Some(name) => {
            println!();
            println!("RESULT: PASS —— 输入视图用 {name} 建得起来 ⇒ VideoProcessor 转换路径可用");
            std::process::exit(0);
        }
        None => {
            println!();
            println!("RESULT: FAIL —— 所有 FourCC 候选都建不出输入视图");
            println!(
                "  ⇒ 本机驱动不支持用 D3D11 VideoProcessor 处理 DXGI_FORMAT_420Opaque。\n\
                 => 转换路径只能走 Pixel Shader View，而 windows 绑定里没有它，需要手写 COM vtable。"
            );
            std::process::exit(1);
        }
    }
}
