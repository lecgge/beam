//! D3D11 VideoProcessor 变体矩阵探针：定位 RGB→NV12 走不通的真正原因。
//!
//! 背景：`beam_encode::color::RgbToNv12` 用 D3D11 VideoProcessor 做 RGB→NV12，
//! 但本机 `CreateVideoProcessorInputView` 返回 `E_INVALIDARG(0x80070057)`。
//! 这个探针把可疑的维度做成矩阵，逐个试，**如实打印每一步的 HRESULT**。
//!
//! 可变维度：
//!
//! | 维度 | 取值 |
//! |---|---|
//! | 输入视图 `FourCC` | DXGI 枚举值（87）/ 真 FOURCC（'W8B8' = 0x38424257）/ 0 |
//! | `ContentDesc.Usage` | PLAYBACK_NORMAL / PROCESSOR / ENCODE / DECODE |
//! | 源纹理绑定 | SHADER_RESOURCE / SHADER_RESOURCE\|VIDEO_ENCODER |
//! | NV12 目标纹理绑定 | VIDEO_ENCODER / VIDEO_ENCODER\|SHADER_RESOURCE |
//!
//! 用法：
//!
//! ```powershell
//! cargo run -p beam-encode --example color_vp_probe --release
//! ```

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoDevice,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BIND_VIDEO_ENCODER, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_USAGE, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

/// DXGI_FORMAT → 真 FOURCC（'W','8','B','8'）。
const FOURCC_B8G8R8A8: u32 = 0x3842_4257;

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
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
    .map_err(|e| format!("{e:?}"))?;
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err("设备或上下文为空".to_string()),
    }
}

fn create_texture(
    device: &ID3D11Device,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    bind: u32,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(
                &desc,
                None,
                Some(&mut texture as *mut Option<ID3D11Texture2D>),
            )
            .map_err(|e| format!("{e:?}"))?;
    }
    texture.ok_or_else(|| "纹理为空".to_string())
}

/// 跑一个组合，返回 (是否成功, 失败原因)。
fn try_variant(
    video_device: &ID3D11VideoDevice,
    src: &ID3D11Texture2D,
    dst: &ID3D11Texture2D,
    _src_format: DXGI_FORMAT,
    fourcc: u32,
    usage: windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_USAGE,
) -> Result<(), String> {
    let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        InputFrameRate: DXGI_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        },
        InputWidth: WIDTH,
        InputHeight: HEIGHT,
        OutputFrameRate: DXGI_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        },
        OutputWidth: WIDTH,
        OutputHeight: HEIGHT,
        Usage: usage,
    };
    let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content) }
        .map_err(|e| format!("枚举器: {e:?}"))?;

    let _processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
        .map_err(|e| format!("处理器: {e:?}"))?;

    // 输入视图
    let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: fourcc,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPIV {
                MipSlice: 0,
                ArraySlice: 0,
            },
        },
    };
    let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorInputView(
                src,
                &enumerator,
                &in_desc,
                Some(&mut input_view as *mut Option<ID3D11VideoProcessorInputView>),
            )
            .map_err(|e| format!("输入视图: {e:?}"))?;
    }
    let _input_view = input_view.ok_or("输入视图为空")?;

    // 输出视图
    let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
        },
    };
    let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorOutputView(
                dst,
                &enumerator,
                &out_desc,
                Some(&mut output_view as *mut Option<ID3D11VideoProcessorOutputView>),
            )
            .map_err(|e| format!("输出视图: {e:?}"))?;
    }
    output_view.ok_or("输出视图为空")?;

    Ok(())
}

fn main() {
    println!("=== D3D11 VideoProcessor 变体矩阵探针 ===");
    let (device, _context) = match create_device() {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: 无法创建 D3D11 设备: {e}");
            std::process::exit(2);
        }
    };
    let video_device: ID3D11VideoDevice = match device.cast() {
        Ok(v) => v,
        Err(e) => {
            println!("FAIL: QueryInterface(ID3D11VideoDevice) 失败: {e:?}");
            std::process::exit(2);
        }
    };
    println!("ID3D11VideoDevice: OK");

    // windows crate 0.62 只导出了三个 D3D11_VIDEO_USAGE 变体，
    // 其余按 MS 的枚举值直接构造（ENCODE=3 / DECODE=4 / PROCESSOR=6）。
    let usages = [
        (
            "PLAYBACK_NORMAL",
            windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        ),
        ("PROCESSOR(6)", D3D11_VIDEO_USAGE(6)),
        ("ENCODE(3)", D3D11_VIDEO_USAGE(3)),
        ("DECODE(4)", D3D11_VIDEO_USAGE(4)),
    ];
    let fourccs = [
        ("DXGI枚举值(87)", DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32),
        ("真FOURCC(W8B8)", FOURCC_B8G8R8A8),
        ("0", 0),
    ];
    let src_binds = [
        ("SRV", D3D11_BIND_SHADER_RESOURCE.0 as u32),
        (
            "SRV|VIDEO_ENCODER",
            D3D11_BIND_SHADER_RESOURCE.0 as u32 | D3D11_BIND_VIDEO_ENCODER.0 as u32,
        ),
    ];
    let dst_binds = [
        ("VIDEO_ENCODER", D3D11_BIND_VIDEO_ENCODER.0 as u32),
        (
            "VIDEO_ENCODER|SRV",
            D3D11_BIND_VIDEO_ENCODER.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ),
    ];

    let mut wins = 0usize;
    for (usage_name, usage) in usages {
        for (fourcc_name, fourcc) in fourccs {
            for (src_name, src_bind) in src_binds {
                for (dst_name, dst_bind) in dst_binds {
                    let src = match create_texture(
                        &device,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        WIDTH,
                        HEIGHT,
                        src_bind,
                    ) {
                        Ok(t) => t,
                        Err(e) => {
                            println!(
                                "usage={usage_name:<16} fourcc={fourcc_name:<16} src={src_name:<18} dst={dst_name:<18} 源纹理建不出来: {e}"
                            );
                            continue;
                        }
                    };
                    let dst = match create_texture(
                        &device,
                        DXGI_FORMAT_NV12,
                        WIDTH,
                        HEIGHT,
                        dst_bind,
                    ) {
                        Ok(t) => t,
                        Err(e) => {
                            println!(
                                "usage={usage_name:<16} fourcc={fourcc_name:<16} src={src_name:<18} dst={dst_name:<18} 目标纹理建不出来: {e}"
                            );
                            continue;
                        }
                    };
                    match try_variant(
                        &video_device,
                        &src,
                        &dst,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        fourcc,
                        usage,
                    ) {
                        Ok(()) => {
                            wins += 1;
                            println!(
                                "usage={usage_name:<16} fourcc={fourcc_name:<16} src={src_name:<18} dst={dst_name:<18} **成功**"
                            );
                        }
                        Err(reason) => {
                            println!(
                                "usage={usage_name:<16} fourcc={fourcc_name:<16} src={src_name:<18} dst={dst_name:<18} {reason}"
                            );
                        }
                    }
                }
            }
        }
    }
    println!();
    println!("可用的组合数: {wins}");
    if wins == 0 {
        println!("结论: 本机 D3D11 VideoProcessor 在所有组合下都建不出输入视图 —— 走 compute shader 方案。");
    }
    // 引用一下，避免未使用告警。
    let _ = (
        D3D11_CPU_ACCESS_READ.0,
        D3D11_MAP_READ.0,
        D3D11_MAPPED_SUBRESOURCE::default(),
    );
}
