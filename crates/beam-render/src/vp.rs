//! D3D11 VideoProcessor 转换路径 —— 4:2:0 解码纹理 → BGRA（§3.10）。
//!
//! ## 为什么需要它
//!
//! D3D11VA 视频解码的原生输出是 `DXGI_FORMAT_420Opaque`（4:2:0 双平面），
//! 而普通桌面窗口的交换链是 `DXGI_FORMAT_B8G8R8A8_UNORM`。
//! B17 的对照实验实测：flip-model 交换链**不接受** 4:2:0 视频格式
//! ⇒ 「解码纹理直接交给交换链」的零拷贝在本机不可行。
//!
//! D3D11 的 Pixel Shader View 在 windows 绑定里**没有**
//! （`ID3D11PixelShaderView` 与 `D3D11_VIEW_DESC` 都不存在），
//! 手写 COM vtable 的代价很高。
//!
//! D3D11 **VideoProcessor** 则是硬件做格式转换的正规途径 —— 本机实测可用。
//!
//! ## 与 `beam-encode` 那次失败的差别（重要）
//!
//! `beam-encode` 的 VP 探针试过 **96 组变体全灭**
//! （`examples/color_vp_probe.rs`），但它的源纹理是**自造的** NV12 / BGRA，
//! 而输入视图的 `FourCC` 取的是 **DXGI 枚举值**或 `'W8B8'`（BGRA 的真 FOURCC）。
//!
//! 本模块的源纹理是**解码器真实输出**的 `DXGI_FORMAT_420Opaque`，
//! 其对应的真 FOURCC 是 **`'420O'`**（`MAKEFOURCC('4','2','0','O')` = `0x4F303234`）
//! —— 那个值在之前那次探针里**从没被试过**。
//!
//! 真机实测（本机 RTX 3070 / 驱动 596.49）：
//!
//! ```text
//! 源纹理: DXGI 格式 0x00000067，1920x1152（对齐后的 coded height）
//! 枚举器: 建成
//! 输入视图: 用 '420O' 建得起来
//! ```
//!
//! ## 高度对齐（重要）
//!
//! 解码纹理高 **1152** 是**对齐后**的 coded height，而可见画面只有 **1080**。
//! 4:2:0 的平面里，Y 平面的**行距等于宽度**（1920 字节），解码器把 1080 行
//! 画面写在前面、后面是 padding ⇒ 转换时必须按**可见高度**采样，
//! 否则底部会多出 72 行未初始化的内容。
//!
//! ## 数据流
//!
//! ```text
//! 解码纹理 (420Opaque, 1920x1152)
//!   └─ VideoProcessor 输入视图 (FourCC='420O')
//!        └─ VideoProcessorBlt ──> BGRA 目标纹理 (1920x1080)
//!                                   └─ CopyResource ──> 交换链后备缓冲
//! ```

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

use crate::error::RenderError;

/// `DXGI_FORMAT_420Opaque` 对应的真 FOURCC：`MAKEFOURCC('4','2','0','O')`。
pub const FOURCC_420_OPAQUE: u32 = 0x4F30_3234;

/// 视频处理器输入视图的 FourCC 候选（按「可能性」排序）。
///
/// 每项是 `(名字, FourCC)`；探针会逐个真机试。
pub const FOURCC_CANDIDATES: &[(&str, u32)] = &[
    ("'420O' (420Opaque 的真 FOURCC)", FOURCC_420_OPAQUE),
    ("DXGI 枚举值 103 (420Opaque)", 103),
    ("'NV12'", 0x3231_564E),
    ("'YV12'", 0x3231_5659),
    ("DXGI 枚举值 70 (NV12)", 70),
];

/// VP 诊断结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpProbe {
    /// 解码纹理的 DXGI 格式（DXGI 枚举值）。
    pub source_format: i32,
    /// 解码纹理的宽。
    pub source_width: u32,
    /// 解码纹理的高（**对齐后**的 coded height）。
    pub source_height: u32,
    /// 枚举器是否建得起来。
    pub enumerator_ok: bool,
    /// 枚举器/处理器失败时的原因。
    pub enumerator_error: Option<String>,
    /// 哪个 FourCC 让输入视图建起来了（`None` = 全灭）。
    pub working_fourcc: Option<&'static str>,
    /// 每个候选的失败原因（名字 → 原因）。
    pub fourcc_errors: Vec<(String, String)>,
}

/// 诊断：给定 4:2:0 解码纹理，VP 能不能把它当输入。
///
/// `source_handle` 是 `DecodedFrame::surface` 的整数值（平台指针），
/// 与 [`crate::d3d11`] 里 `present` 的约定一致。
#[allow(
    unsafe_code,
    reason = "VideoProcessor 的枚举器/处理器/输入视图创建都是 unsafe 的 D3D11 COM 调用"
)]
pub fn probe_video_processor(source_handle: usize) -> Result<VpProbe, String> {
    let raw = source_handle as *mut core::ffi::c_void;
    if raw.is_null() {
        return Err("没有解码纹理（句柄为空）".to_string());
    }
    // SAFETY: 借用调用方持有的 COM 指针，不接管所有权。
    let source: &ID3D11Texture2D =
        match unsafe { windows::core::Interface::from_raw_borrowed(&raw) } {
            Some(t) => t,
            None => return Err("无法借用解码纹理".to_string()),
        };

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` 是合法的输出参数。
    unsafe { source.GetDesc(&mut desc) };

    let device: ID3D11Device =
        unsafe { source.GetDevice() }.map_err(|e| format!("读不到设备: {e:?}"))?;
    let context =
        unsafe { device.GetImmediateContext() }.map_err(|e| format!("读不到上下文: {e:?}"))?;
    let video_device: ID3D11VideoDevice = device
        .cast()
        .map_err(|_| "D3D11 VideoDevice 接口不可用(QueryInterface 失败)".to_string())?;
    let _video_context: ID3D11VideoContext = context
        .cast()
        .map_err(|_| "D3D11 VideoContext 接口不可用(QueryInterface 失败)".to_string())?;

    let mut probe = VpProbe {
        source_format: desc.Format.0,
        source_width: desc.Width,
        source_height: desc.Height,
        enumerator_ok: false,
        enumerator_error: None,
        working_fourcc: None,
        fourcc_errors: Vec::new(),
    };

    // 枚举器：按「宽高 + 帧格式」枚举支持的组合，**不带**输入格式。
    let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        InputFrameRate: DXGI_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        },
        InputWidth: desc.Width,
        InputHeight: desc.Height,
        OutputFrameRate: DXGI_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        },
        OutputWidth: desc.Width,
        OutputHeight: desc.Height,
        Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    };
    let enumerator: ID3D11VideoProcessorEnumerator =
        match unsafe { video_device.CreateVideoProcessorEnumerator(&content) } {
            Ok(e) => {
                probe.enumerator_ok = true;
                e
            }
            Err(err) => {
                probe.enumerator_error = Some(format!("枚举器创建失败: {err:?}"));
                return Ok(probe);
            }
        };

    let _processor: ID3D11VideoProcessor =
        match unsafe { video_device.CreateVideoProcessor(&enumerator, 0) } {
            Ok(p) => p,
            Err(err) => {
                probe.enumerator_error = Some(format!("处理器创建失败: {err:?}"));
                return Ok(probe);
            }
        };

    // 输入视图：FourCC 候选矩阵，逐个真机试。
    for (name, fourcc) in FOURCC_CANDIDATES {
        match create_input_view(&video_device, &enumerator, source, *fourcc) {
            Ok(_) => {
                probe.working_fourcc = Some(name);
                break;
            }
            Err(err) => probe.fourcc_errors.push((name.to_string(), err)),
        }
    }

    Ok(probe)
}

/// 源纹理的 DXGI 格式是否 4:2:0 视频格式（`NV12` / `420Opaque`）。
pub fn is_420_format(format: DXGI_FORMAT) -> bool {
    format.0 == 70 || format.0 == 103
}

/// 建 VideoProcessor 的输入视图。
#[allow(
    unsafe_code,
    reason = "CreateVideoProcessorInputView 是 unsafe 的 D3D11 COM 调用"
)]
fn create_input_view(
    video_device: &ID3D11VideoDevice,
    enumerator: &ID3D11VideoProcessorEnumerator,
    src: &ID3D11Texture2D,
    fourcc: u32,
) -> Result<ID3D11VideoProcessorInputView, String> {
    let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: fourcc,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPIV {
                MipSlice: 0,
                ArraySlice: 0,
            },
        },
    };
    let mut view: Option<ID3D11VideoProcessorInputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorInputView(
                src,
                enumerator,
                &desc,
                Some(&mut view as *mut Option<ID3D11VideoProcessorInputView>),
            )
            .map_err(|e| format!("{e:?}"))?;
    }
    view.ok_or_else(|| "输入视图为空".to_string())
}

/// 4:2:0 解码纹理 → BGRA 的 VideoProcessor 转换器。
///
/// 生命周期与纹理绑定：所有纹理必须来自**同一个** `ID3D11Device`，
/// 否则 VideoProcessor 无法在源与目标之间做 blit。
#[derive(Debug)]
pub struct VpConverter {
    /// 设备（保活：源与目标纹理都在它上面）。
    _device: ID3D11Device,
    /// 立即上下文。
    _context: ID3D11DeviceContext,
    /// `ID3D11VideoDevice`。
    video_device: ID3D11VideoDevice,
    /// `ID3D11VideoContext`：`VideoProcessorBlt` 要它。
    video_context: ID3D11VideoContext,
    /// 枚举器。
    enumerator: ID3D11VideoProcessorEnumerator,
    /// 处理器。
    processor: ID3D11VideoProcessor,
    /// BGRA 目标纹理。
    target: ID3D11Texture2D,
    /// 目标宽。
    width: u32,
    /// 目标高。
    height: u32,
    /// 建立时源纹理的格式（变了要重建）。
    src_format: DXGI_FORMAT,
    /// 建立时源纹理的高（变了要重建）。
    src_height: u32,
}

impl VpConverter {
    /// 针对给定的 4:2:0 源纹理建立转换器。
    ///
    /// `visible_height` 是**可见**画面高度（解码器报告的高度），
    /// 与纹理自身的对齐后高度可能不同（4:2:0 的 Y 平面行距 = 宽度，
    /// 解码器把画面写在前面、后面是 padding）。
    #[allow(
        unsafe_code,
        reason = "VideoProcessor 的创建与纹理创建都是 unsafe 的 D3D11 COM 调用"
    )]
    pub fn new(src: &ID3D11Texture2D, visible_height: u32) -> Result<Self, RenderError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut desc) };
        if !is_420_format(desc.Format) {
            return Err(RenderError::SourceMismatch {
                reason: format!(
                    "源纹理格式 0x{:08x} 不是 4:2:0 视频格式,不需要 VideoProcessor",
                    desc.Format.0
                ),
            });
        }
        let device = unsafe { src.GetDevice() }.map_err(|e| RenderError::SwapChainFailed {
            reason: format!("读不到解码纹理的设备: {e:?}"),
        })?;
        let context =
            unsafe { device.GetImmediateContext() }.map_err(|e| RenderError::SwapChainFailed {
                reason: format!("读不到设备上下文: {e:?}"),
            })?;
        let video_device: ID3D11VideoDevice =
            device.cast().map_err(|_| RenderError::SwapChainFailed {
                reason: "D3D11 VideoDevice 接口不可用(QueryInterface 失败)".to_string(),
            })?;
        let video_context: ID3D11VideoContext =
            context.cast().map_err(|_| RenderError::SwapChainFailed {
                reason: "D3D11 VideoContext 接口不可用(QueryInterface 失败)".to_string(),
            })?;

        // 枚举器：宽高用**可见**尺寸（输出也是可见尺寸）。
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: desc.Width,
            InputHeight: visible_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: desc.Width,
            OutputHeight: visible_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator =
            unsafe { video_device.CreateVideoProcessorEnumerator(&content) }.map_err(|e| {
                RenderError::SwapChainFailed {
                    reason: format!("VideoProcessor 枚举器创建失败: {e:?}"),
                }
            })?;
        let processor =
            unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }.map_err(|e| {
                RenderError::SwapChainFailed {
                    reason: format!("VideoProcessor 创建失败: {e:?}"),
                }
            })?;

        // 目标纹理：BGRA + 可作渲染目标（后备缓冲要拷进来）。
        let target = create_bgra_texture(&device, desc.Width, visible_height)?;

        Ok(Self {
            _device: device,
            _context: context,
            video_device,
            video_context,
            enumerator,
            processor,
            target,
            width: desc.Width,
            height: visible_height,
            src_format: desc.Format,
            src_height: desc.Height,
        })
    }

    /// 把 4:2:0 源纹理转换成 BGRA，返回转换结果纹理。
    ///
    /// 源纹理格式或高度变了必须重建（D3D11 的输入视图与枚举器绑定）。
    #[allow(
        unsafe_code,
        reason = "VideoProcessorBlt 与输出视图创建都是 unsafe 的 D3D11 COM 调用"
    )]
    pub fn convert(&mut self, src: &ID3D11Texture2D) -> Result<&ID3D11Texture2D, RenderError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut desc) };
        if desc.Format != self.src_format || desc.Height != self.src_height {
            return Err(RenderError::SourceMismatch {
                reason: format!(
                    "解码纹理变了(格式 0x{:08x}/高 {}，原来是 0x{:08x}/高 {})，需要重建转换器",
                    desc.Format.0, desc.Height, self.src_format.0, self.src_height
                ),
            });
        }

        let input_view = create_input_view(
            &video_device_of(&self.video_device),
            &self.enumerator,
            src,
            FOURCC_420_OPAQUE,
        )
        .map_err(|e| RenderError::SwapChainFailed {
            reason: format!("创建 VideoProcessor 输入视图失败: {e}"),
        })?;
        let output_view =
            create_output_view(&self.video_device, &self.enumerator, &self.target, src)?;
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: windows::core::BOOL(1),
            OutputIndex: 0,
            pInputSurface: std::mem::ManuallyDrop::new(Some(input_view)),
            ..Default::default()
        };
        unsafe {
            self.video_context
                .VideoProcessorBlt(&self.processor, &output_view, 0, &[stream])
        }
        .map_err(|e| RenderError::SwapChainFailed {
            reason: format!("VideoProcessorBlt 失败: {e:?}"),
        })?;
        Ok(&self.target)
    }

    /// 目标 BGRA 纹理（转换结果）。
    pub fn target(&self) -> &ID3D11Texture2D {
        &self.target
    }

    /// 目标宽高。
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// 取回 `ID3D11VideoDevice`（`create_input_view` 需要它）。
fn video_device_of(device: &ID3D11VideoDevice) -> ID3D11VideoDevice {
    device.clone()
}

/// 建 VideoProcessor 的输出视图。
#[allow(
    unsafe_code,
    reason = "CreateVideoProcessorOutputView 是 unsafe 的 D3D11 COM 调用"
)]
fn create_output_view(
    video_device: &ID3D11VideoDevice,
    enumerator: &ID3D11VideoProcessorEnumerator,
    target: &ID3D11Texture2D,
    src: &ID3D11Texture2D,
) -> Result<ID3D11VideoProcessorOutputView, RenderError> {
    let mut src_desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { src.GetDesc(&mut src_desc) };
    // 输出视图描述**不带** FourCC（输出格式由目标纹理自己决定）。
    let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
        },
    };
    let mut view: Option<ID3D11VideoProcessorOutputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorOutputView(
                target,
                enumerator,
                &desc,
                Some(&mut view as *mut Option<ID3D11VideoProcessorOutputView>),
            )
            .map_err(|e| RenderError::SwapChainFailed {
                reason: format!("创建 VideoProcessor 输出视图失败: {e:?}"),
            })?;
    }
    view.ok_or(RenderError::SwapChainFailed {
        reason: "VideoProcessor 输出视图为空".to_string(),
    })
}

/// 建一张 BGRA 目标纹理（带渲染目标绑定，可被拷进交换链后备缓冲）。
#[allow(unsafe_code, reason = "CreateTexture2D 是 unsafe 的 D3D11 COM 调用")]
fn create_bgra_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, RenderError> {
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
        // VP 输出视图的目标纹理要能当渲染目标（转完再拷进交换链后备缓冲）。
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 as u32) | (D3D11_BIND_SHADER_RESOURCE.0 as u32),
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
            .map_err(|e| RenderError::SwapChainFailed {
                reason: format!("创建 BGRA 目标纹理失败: {e:?}"),
            })?;
    }
    texture.ok_or(RenderError::SwapChainFailed {
        reason: "BGRA 目标纹理为空".to_string(),
    })
}
