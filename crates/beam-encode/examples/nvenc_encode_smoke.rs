//! 真机 NVENC 出码流验证（§9 M-1 验证点②）。
//!
//! 这个入口**真的编码**：造一张 D3D11 纹理 → 走 GPU 颜色转换 → 送进 NVENC →
//! 拿到真实码流 → 再用软件解码器把码流解回来，核对
//!
//! - 编码出的帧数；
//! - 码流总字节数；
//! - 是否含关键帧（IDR）；
//! - 解码出来的分辨率是否等于输入分辨率。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"   # ffmpeg 的第三方运行库不在 conda 包的 bin 里
//! cargo run -p beam-encode --example nvenc_encode_smoke --features ffmpeg
//! ```
//!
//! 输入源有两条路径，都会被尝试：
//!
//! 1. **RGBA 纹理**（走 D3D11 VideoProcessor 的 RGB→NV12，§3.6 职责第一项）；
//! 2. **NV12 纹理**（跳过颜色转换，直接验证 D3D11 硬件帧 → NVENC）。
//!
//! 哪条路径跑通、失败在哪一步，都会被如实打印。

use beam_encode::{CapturedFrame, EncodeConfig, EncoderKind, HardwareVideoEncoder, VideoEncoder};
use beam_proto::{CodecKind, EncodedFrame};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VIDEO_ENCODER, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC,
};

/// 输入分辨率。
const WIDTH: u32 = 1920;
/// 输入分辨率。
const HEIGHT: u32 = 1080;
/// 编码帧数。
const FRAMES: u32 = 10;
/// 目标码率。
const BITRATE: u32 = 8_000_000;
/// 帧率。
const FPS: u32 = 60;

fn main() {
    println!("=== beam-encode 真机 NVENC 出码流验证 ===");

    let (device, _context) = match create_device() {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: 无法创建 D3D11 设备：{e}");
            std::process::exit(2);
        }
    };
    println!("D3D11 设备: 创建成功");

    probe_hw_device_ctx(&device);

    let mut ok = false;

    // 路径 1：RGBA 纹理 → D3D11 VideoProcessor（RGB→NV12）→ NVENC。
    println!();
    println!("--- 路径 1: RGBA 源纹理（走 VideoProcessor 颜色转换）---");
    match create_rgba_texture(&device) {
        Ok(tex) => {
            ok |= run_encode("RGBA/VideoProcessor", tex);
        }
        Err(e) => println!("FAIL: 无法创建 RGBA 纹理：{e}"),
    }

    // 路径 2：NV12 纹理 → D3D11 硬件帧 → NVENC。
    println!();
    println!("--- 路径 2: NV12 源纹理（直接走 D3D11 硬件帧）---");
    match create_nv12_texture(&device) {
        Ok(tex) => {
            ok |= run_encode("NV12/硬件帧", tex);
        }
        Err(e) => println!("FAIL: 无法创建 NV12 纹理：{e}"),
    }

    println!();
    if ok {
        println!("RESULT: PASS —— NVENC 真的产出了码流，并被软件解码器解回。");
    } else {
        println!("RESULT: FAIL —— 两条路径都没能产出可验证的码流（见上面的具体错误）。");
        std::process::exit(1);
    }
    println!("[teardown] 释放 D3D11 设备…");
    drop(device);
    println!("[teardown] 完成");
    // 显式退出：跳过 C 运行时在 main 返回后的收尾（DLL 卸载阶段由 ExitProcess 触发）。
    std::process::exit(0);
}

/// 跑一轮「编码 N 帧 → 校验码流」。
fn run_encode(label: &str, tex: ID3D11Texture2D) -> bool {
    let cfg = EncodeConfig {
        encoder: EncoderKind::Nvenc,
        codec: CodecKind::Hevc,
        bitrate_bps: BITRATE,
        fps: FPS,
        slices: 0,
        sfe: false,
        idr_interval: 0,
    };
    let mut enc = match HardwareVideoEncoder::open(cfg) {
        Ok(e) => e,
        Err(e) => {
            println!(
                "FAIL[{label}]: 打开编码器失败 {} {} — {}",
                e.code_str(),
                e.code(),
                e.text()
            );
            return false;
        }
    };
    println!("[{label}] 编码器上下文: 建立成功");

    let mut out: Vec<EncodedFrame> = Vec::new();
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
            Ok(encoded) => out.push(encoded),
            // 硬件编码器有一帧延迟：首次调用还没有输出是正常状态。
            Err(e) if e.code() == 6005 => buffering += 1,
            Err(e) => {
                println!(
                    "FAIL[{label}]: 第 {i} 帧编码失败 {} {} — {e}",
                    e.code_str(),
                    e.code()
                );
                return false;
            }
        }
    }
    if let Some(last) = enc.flush(u64::from(FRAMES) * 1_000_000 / u64::from(FPS)) {
        out.push(last);
    }

    // ---- 校验 ----
    let total_bytes: usize = out.iter().map(|f| f.to_payload().len()).sum();
    let key_frames = out.iter().filter(|f| f.is_key).count();
    println!(
        "[{label}] 编码输出: {} 帧, {} 字节, 关键帧 {} 个, 编码器缓冲中 {} 次",
        out.len(),
        total_bytes,
        key_frames,
        buffering
    );

    let mut dims = None;
    for f in &out {
        if let Some(d) = decode_dimensions(&f.to_payload(), CodecKind::Hevc) {
            dims = Some(d);
            break;
        }
    }

    let mut pass = true;
    if out.is_empty() {
        println!("FAIL[{label}]: 没有产出任何码流");
        pass = false;
    }
    if total_bytes == 0 {
        println!("FAIL[{label}]: 码流字节数为 0");
        pass = false;
    }
    if key_frames == 0 {
        println!("FAIL[{label}]: 码流里没有关键帧");
        pass = false;
    }
    match dims {
        Some((w, h)) => {
            println!("[{label}] 软件解码器解出分辨率: {w}x{h}");
            if w != WIDTH || h != HEIGHT {
                println!("FAIL[{label}]: 分辨率不符，期望 {WIDTH}x{HEIGHT}");
                pass = false;
            }
        }
        None => {
            println!("FAIL[{label}]: 软件解码器没能解出分辨率");
            pass = false;
        }
    }
    if pass {
        println!("PASS[{label}]");
    }
    // 把码流落盘，供 beam-decode 的 D3D11VA 验证入口复用
    // （长度前缀的访问单元序列：每条 [u32 len][payload]）。
    let path = dump_stream(label, &out);
    if let Some(p) = path {
        println!("[{label}] 码流已写入: {}", p.display());
    }
    println!("[{label}] 释放编码器…");
    drop(enc);
    println!("[{label}] 编码器已释放");
    pass
}

/// 把编码结果按「长度前缀的访问单元」写到临时文件。
fn dump_stream(label: &str, frames: &[EncodedFrame]) -> Option<std::path::PathBuf> {
    let mut path = std::env::temp_dir();
    path.push("beam-nvenc-smoke.bin");
    let mut buf: Vec<u8> = Vec::new();
    for f in frames {
        let payload = f.to_payload();
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&payload);
    }
    match std::fs::write(&path, &buf) {
        Ok(()) => Some(path),
        Err(e) => {
            println!("[{label}] 写码流文件失败: {e}");
            None
        }
    }
}

/// 诊断：ffmpeg 的 `av_hwdevice_ctx_alloc(D3D11VA)` 到底把什么放进了 `hwctx`。
fn probe_hw_device_ctx(device: &ID3D11Device) {
    use ffmpeg_next::sys as ffi;
    use windows::core::Interface;
    unsafe {
        let mut dev = ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
        if dev.is_null() {
            println!("[diag] av_hwdevice_ctx_alloc 返回 NULL");
            return;
        }
        let hwdev = (*dev).data as *mut ffi::AVHWDeviceContext;
        println!(
            "[diag] alloc 后: hwctx = {:?}  user_opaque = {:?}  free = {:?}",
            (*hwdev).hwctx,
            (*hwdev).user_opaque,
            (*hwdev).free
        );
        let base = (*hwdev).hwctx as *const *const std::ffi::c_void;
        println!(
            "[diag] alloc 后 hwctx[0]=device={:?} hwctx[1]=device_context={:?} hwctx[2]=video_device={:?}",
            *base, *base.add(1), *base.add(2)
        );

        // 按 FFmpeg 头文件的约定：用户填 device，然后 init。
        let owned = device.clone();
        let base_mut = (*hwdev).hwctx as *mut *mut std::ffi::c_void;
        *base_mut = owned.as_raw();
        let ret = ffi::av_hwdevice_ctx_init(dev);
        println!(
            "[diag] av_hwdevice_ctx_init -> {ret}, 之后 free = {:?}, user_opaque = {:?}",
            (*hwdev).free,
            (*hwdev).user_opaque
        );
        println!(
            "[diag] init 后 hwctx[0]=device={:?} hwctx[1]=device_context={:?} hwctx[2]=video_device={:?}",
            *base, *base.add(1), *base.add(2)
        );
        let ret = ffi::av_hwdevice_ctx_init(dev);
        println!("[diag] 再次 init -> {ret}（0 表示幂等可用）");

        std::mem::forget(owned);
        ffi::av_buffer_unref(&mut dev);
        println!("[diag] 释放设备上下文：未崩溃");
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

/// 创建一张 RGBA 纹理（VideoProcessor 的输入源）。
///
/// 内容对「能不能出码流」没有影响，所以这里用普通纹理（`D3D11_USAGE_DEFAULT`），
/// 不去折腾不可变纹理与初始数据。
fn create_rgba_texture(device: &ID3D11Device) -> Result<ID3D11Texture2D, String> {
    create_texture(
        device,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        D3D11_USAGE_DEFAULT,
        None,
    )
}

/// 创建一张 NV12 纹理（内容无关紧要，只验证「D3D11 硬件帧 → NVENC」这条链路）。
fn create_nv12_texture(device: &ID3D11Device) -> Result<ID3D11Texture2D, String> {
    create_texture(device, DXGI_FORMAT_NV12, D3D11_USAGE_DEFAULT, None)
}

/// 按描述符创建纹理；bind flags 逐档降级并报告实际结果。
fn create_texture(
    device: &ID3D11Device,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
    initial: Option<&D3D11_SUBRESOURCE_DATA>,
) -> Result<ID3D11Texture2D, String> {
    let flag_candidates = [
        (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_VIDEO_ENCODER.0) as u32,
        D3D11_BIND_VIDEO_ENCODER.0 as u32,
        D3D11_BIND_SHADER_RESOURCE.0 as u32,
        0u32,
    ];
    let mut last = String::new();
    for bind in flag_candidates {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: WIDTH,
            Height: HEIGHT,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        let res = unsafe {
            device.CreateTexture2D(
                &desc,
                initial.map(|i| i as *const D3D11_SUBRESOURCE_DATA),
                Some(&mut texture as *mut Option<ID3D11Texture2D>),
            )
        };
        match res {
            Ok(()) => {
                println!(
                    "  CreateTexture2D(format={:?}, bind=0x{bind:x}) -> OK",
                    format.0
                );
                if let Some(t) = texture {
                    return Ok(t);
                }
            }
            Err(e) => {
                println!(
                    "  CreateTexture2D(format={:?}, bind=0x{bind:x}) -> ERR {e:?}",
                    format.0
                );
                last = format!("{e:?}");
            }
        }
    }
    Err(last)
}

/// 用软件解码器把一段码流解回来，读出分辨率。
fn decode_dimensions(payload: &[u8], codec: CodecKind) -> Option<(u32, u32)> {
    use ffmpeg_next::sys as ffi;
    use std::ffi::CString;

    let name = match codec {
        CodecKind::Hevc => "hevc",
        CodecKind::H264 => "h264",
        CodecKind::Av1 => "av1",
    };
    let cname = CString::new(name).ok()?;
    let decoder = unsafe { ffi::avcodec_find_decoder_by_name(cname.as_ptr()) };
    if decoder.is_null() {
        return None;
    }
    let mut ctx = unsafe { ffi::avcodec_alloc_context3(decoder) };
    if ctx.is_null() {
        return None;
    }
    let ret = unsafe { ffi::avcodec_open2(ctx, decoder, std::ptr::null_mut()) };
    if ret < 0 {
        unsafe { ffi::avcodec_free_context(&mut ctx) };
        return None;
    }

    let mut pkt = unsafe { ffi::av_packet_alloc() };
    let ret = unsafe { ffi::av_new_packet(pkt, payload.len() as i32) };
    if ret < 0 {
        unsafe {
            ffi::av_packet_free(&mut pkt);
            ffi::avcodec_free_context(&mut ctx);
        }
        return None;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(payload.as_ptr(), (*pkt).data, payload.len());
    }
    let ret = unsafe { ffi::avcodec_send_packet(ctx, pkt) };
    unsafe {
        ffi::av_packet_free(&mut pkt);
    }
    if ret < 0 {
        unsafe { ffi::avcodec_free_context(&mut ctx) };
        return None;
    }

    let mut frame = unsafe { ffi::av_frame_alloc() };
    let ret = unsafe { ffi::avcodec_receive_frame(ctx, frame) };
    let dims = if ret >= 0 {
        Some(
            (unsafe { (*frame).width } as u32, unsafe { (*frame).height }
                as u32),
        )
    } else {
        None
    };
    unsafe {
        ffi::av_frame_free(&mut frame);
        ffi::avcodec_free_context(&mut ctx);
    }
    dims
}
