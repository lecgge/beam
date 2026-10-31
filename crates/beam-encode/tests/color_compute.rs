//! compute shader 颜色转换的**真机**验证（§3.6 / E-01）。
//!
//! ## 验证什么
//!
//! 1. **色彩正确性**：造一张已知图案的 BGRA8 纹理 → 走 compute shader 转 NV12 →
//!    回读两个平面 → 与 **CPU 参考实现**逐像素比对（BT.709 + limited range）。
//!    这条能抓住「通道顺序搞反」「矩阵系数写错」「UV 平面没做 2×2 下采样」
//!    这类**看起来能跑、但颜色是错的**问题。
//! 2. **耗时**：1080p / 4K 各跑若干帧，给出单帧耗时（对照 §3.6 的
//!    「颜色转换 + 编码 T1 ≤ 7 ms」）。
//!
//! ## 运行环境
//!
//! 需要 Windows + D3D11 设备（本机 RTX 3070）。不依赖 ffmpeg，
//! 因此可以在**默认构建**下跑（`color_compute` 模块不受 `ffmpeg` feature 门控）。
//!
//! ```powershell
//! cargo test -p beam-encode --test color_compute --release -- --nocapture
//! ```

#![cfg(windows)]

use std::time::{Duration, Instant};

use beam_encode::color_compute::{read_back_nv12_compute, ComputeConverter};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_WRITE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

fn create_device() -> Option<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
            windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .ok()?;
    Some((device?, context?))
}

/// 造一张 BGRA8 纹理：每个像素的 R/G/B 由坐标决定（便于肉眼核对通道顺序）。
fn create_bgra_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> (ID3D11Texture2D, Vec<u8>) {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = (y * width as usize + x) * 4;
            // BGRA 顺序：内存里是 B、G、R、A。
            pixels[i] = (x % 256) as u8;
            pixels[i + 1] = (y % 256) as u8;
            pixels[i + 2] = ((x + y) % 256) as u8;
            pixels[i + 3] = 255;
        }
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
    };
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr() as *const core::ffi::c_void,
        SysMemPitch: width * 4,
        SysMemSlicePitch: height * width * 4,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(
                &desc,
                Some(&initial),
                Some(&mut texture as *mut Option<ID3D11Texture2D>),
            )
            .expect("应当能创建 BGRA8 纹理");
    }
    (texture.expect("纹理"), pixels)
}

/// CPU 参考实现：BT.709 + limited range（与 HLSL 一一对应）。
fn reference_yuv(pixels: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>) {
    let mut y_plane = vec![0u8; (width as usize) * (height as usize)];
    let mut uv_plane = vec![0u8; (width as usize) * (height as usize / 2)];
    for row in 0..height as usize {
        for col in 0..width as usize {
            let i = (row * width as usize + col) * 4;
            let b = pixels[i] as f32 / 255.0;
            let g = pixels[i + 1] as f32 / 255.0;
            let r = pixels[i + 2] as f32 / 255.0;
            let yy = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let u = (b - yy) / 1.8556;
            let v = (r - yy) / 1.5748;
            y_plane[row * width as usize + col] = quantize(16.0 + 219.0 * yy);
            // NV12：每个 2×2 块取左上角像素，UV 交织。
            // UV 平面是 width/2 个 R8G8 采样 ⇒ 每行 width 字节。
            if row % 2 == 0 && col % 2 == 0 {
                let uv_i = (row / 2) * width as usize + col;
                uv_plane[uv_i] = quantize(128.0 + 112.0 * u);
                uv_plane[uv_i + 1] = quantize(128.0 + 112.0 * v);
            }
        }
    }
    (y_plane, uv_plane)
}

/// 与 GPU 一致的量化：`float` → R8_UNORM ⇒ `round(clamp(v,0,1) * 255)`。
fn quantize(v: f32) -> u8 {
    let clamped = v.clamp(0.0, 1.0);
    (clamped * 255.0).round() as u8
}

fn max_diff(a: &[u8], b: &[u8]) -> (u8, usize) {
    assert_eq!(a.len(), b.len(), "平面长度不同");
    let mut worst = 0u8;
    let mut worst_at = 0usize;
    let mut over = 0usize;
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        let d = x.abs_diff(*y);
        if d > 2 {
            over += 1;
        }
        if d > worst {
            worst = d;
            worst_at = i;
        }
    }
    let _ = worst_at;
    (worst, over)
}

#[test]
fn compute_shader_matches_the_cpu_reference() {
    // 没有硬件 D3D11 设备（CI 的机器可能只有软件适配器）⇒ 跳过,不判失败。
    let Some((device, context)) = create_device() else {
        println!("跳过:本机没有可用的硬件 D3D11 设备");
        return;
    };
    let width = 320u32;
    let height = 240u32;
    let (texture, pixels) = create_bgra_texture(&device, width, height);

    // 校验用**不带 VIDEO_ENCODER** 的那一张（带了的拷不出来）。
    let mut converter =
        ComputeConverter::new_readback(&texture).expect("应当能建出 compute 转换器");
    let nv12 = converter.convert(&texture).expect("转换应当成功");

    let y_plane = read_back_nv12_compute(&device, &context, &nv12).expect("应当能回读");
    let (ref_y, ref_uv) = reference_yuv(&pixels, width, height);

    // Y 平面：直接从 NV12 纹理读回（NV12 staging 的平面 0 可映射）。
    let (y_worst, y_over) = max_diff(&y_plane, &ref_y);
    assert_eq!(y_over, 0, "Y 平面与 CPU 参考实现偏差过大");

    // UV 平面：本机驱动读不回 NV12 的平面 1（staging 平面 1 报 E_INVALIDARG、
    // 平面拷贝读出来全 0），因此用**影子平面**（同一着色器写出的普通 R8G8 纹理）
    // 校验色度计算。
    let shadow = converter
        .read_back_uv_shadow(&device, &context)
        .expect("应当能读回影子平面");
    let (uv_worst, uv_over) = max_diff(&shadow, &ref_uv);
    println!(
        "Y 平面(自 NV12 读回): 最大偏差 {y_worst}（超 ±2 的像素 {y_over} 个）; \
         UV(影子平面): 最大偏差 {uv_worst}（超 ±2 的像素 {uv_over} 个）"
    );
    assert_eq!(uv_over, 0, "影子平面的色度与 CPU 参考实现偏差过大");
}

#[test]
fn conversion_timing_at_1080p_and_4k() {
    let Some((device, _context)) = create_device() else {
        println!("跳过:本机没有可用的硬件 D3D11 设备");
        return;
    };
    for (name, width, height) in [("1080p", 1920u32, 1080u32), ("4K", 3840u32, 2160u32)] {
        let (texture, _pixels) = create_bgra_texture(&device, width, height);
        let mut converter =
            ComputeConverter::new_readback(&texture).expect("应当能建出 compute 转换器");
        // 预热（首次调用要建 SRV / 常量缓冲）。
        let _ = converter.convert(&texture).expect("预热转换");

        const ROUNDS: u32 = 30;
        let mut best = Duration::MAX;
        let mut total = Duration::ZERO;
        for _ in 0..ROUNDS {
            let t0 = Instant::now();
            let _ = converter.convert(&texture).expect("转换");
            // 等 GPU 真正跑完 —— 否则测到的只是「把命令塞进队列」的时间。
            converter.wait_idle().expect("等待 GPU");
            let d = t0.elapsed();
            total += d;
            if d < best {
                best = d;
            }
        }
        let avg = total / ROUNDS;
        println!(
            "{name} ({width}x{height}): 单帧颜色转换 最快 {:.3} ms / 平均 {:.3} ms（§3.6：颜色转换 + 编码 T1 ≤ 7 ms）",
            best.as_secs_f64() * 1e3,
            avg.as_secs_f64() * 1e3
        );
    }
}
