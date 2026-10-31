//! GPU 上的 RGB→NV12 颜色转换（§3.6 职责第一项）。
//!
//! §3.6：「GPU 上 RGB→NV12（**D3D11 VideoProcessor 或 compute shader**）」——
//! 两条路都被文档允许。本模块提供**两条**后端：
//!
//! 1. **D3D11 VideoProcessor**（本模块的 [`VideoProcessorPath`]）：
//!    抓屏纹理（RGBA/BGRA）作为 VideoProcessor 的输入源；
//!    一张 NV12 纹理作为输出目标；
//!    `VideoProcessorBlt` 完成缩放 + 色彩空间转换，全程留在 GPU 上。
//!
//! 2. **compute shader**（[`crate::color_compute`]）：驱动不支持 VideoProcessor 时回退。
//!
//! ## 本机的实际情况
//!
//! 本机（RTX 3070 / 驱动 596.49）的 VideoProcessor **建不出输入视图**
//! （`CreateVideoProcessorInputView` 恒返回 `E_INVALIDARG`）；
//! 为排除「参数写错」，做过 96 组参数变体矩阵（见 `examples/color_vp_probe.rs`），
//! **一组都没建出来** ⇒ 是驱动侧的能力缺失，不是参数问题。
//!
//! 因此对外的门面 [`RgbToNv12`] 是「VideoProcessor 优先、建不出来就回退
//! compute shader」，两条后端产出的 NV12 像素口径一致（BT.709 + limited range）。
//!
//! ## 转换结果有两种用法
//!
//! - **D3D11 硬件帧路径**：NV12 纹理与 ffmpeg 帧池在同一个 `ID3D11Device` 上，
//!   可以直接 `CopyResource` 进编码器的帧（零拷贝，§3.6「D3D11 硬件帧上下文」）。
//! - **NV12 上传路径**：编码器不支持 `AV_PIX_FMT_D3D11` 时（例如部分 QSV 构建），
//!   把 NV12 纹理回读到内存，按 `AV_PIX_FMT_NV12` 送帧。
//!
//! **验证状态**：compute shader 路径已在本机真机验证 ——
//! Y 平面**逐字节**等于 CPU 参考实现（BT.709 + limited range），
//! 耗时 1080p 约 1.0 ms、4K 约 2.1 ms（单帧，含 GPU 同步）。

use std::ffi::c_void;
use std::mem::ManuallyDrop;

use windows::core::{Interface, BOOL};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VIDEO_ENCODER,
    D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEX2D_VPIV,
    D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

use crate::error::EncError;

/// GPU 上的 RGB→NV12 转换器（基于 D3D11 VideoProcessor）。
///
/// 生命周期与抓屏纹理绑定：所有纹理必须来自**同一个** `ID3D11Device`，
/// 否则 VideoProcessor 无法在源与目标之间做 blit。
pub(crate) struct VideoProcessorPath {
    /// 源纹理所在的设备。
    ///
    /// 保留这份引用是为了让设备活得比 VideoProcessor / 视图更久。
    _device: ID3D11Device,
    /// 设备的立即上下文（同样只为保活）。
    _context: ID3D11DeviceContext,
    /// VideoProcessor 枚举器（决定支持的输入/输出格式组合）。
    ///
    /// 源纹理本来就是 NV12 时为 `None`（无需转换）。
    enumerator: Option<ID3D11VideoProcessorEnumerator>,
    /// VideoProcessor 实例。
    processor: Option<ID3D11VideoProcessor>,
    /// NV12 输出视图。
    output_view: Option<ID3D11VideoProcessorOutputView>,
    /// `ID3D11VideoDevice`：创建枚举器/处理器/输入输出视图都要它。
    ///
    /// 通过 `QueryInterface` 从 `ID3D11Device` 上取（D3D11 的视频接口）。
    video_device: Option<ID3D11VideoDevice>,
    /// `ID3D11VideoContext`：`VideoProcessorBlt` 要它。
    video_context: Option<ID3D11VideoContext>,
    /// NV12 目标纹理。
    nv12: ID3D11Texture2D,
    /// 目标宽。
    width: u32,
    /// 目标高。
    height: u32,
    /// 建立时源纹理的格式（变化时需要重建 VideoProcessor）。
    src_format: DXGI_FORMAT,
}

impl std::fmt::Debug for VideoProcessorPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoProcessorPath")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl VideoProcessorPath {
    /// 针对给定的源纹理建立转换器。
    ///
    /// 源纹理已经是 NV12 时不会建立 VideoProcessor（无需转换），
    /// 但仍会返回可用的转换器，`convert` 直接透传源纹理。
    #[allow(
        unsafe_code,
        reason = "需要调用 D3D11 COM 接口创建纹理/VideoProcessor/视图，这些调用全部是 unsafe"
    )]
    pub fn new(src: &ID3D11Texture2D) -> Result<Self, EncError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut desc) };

        let device = unsafe { src.GetDevice() }.map_err(|_| EncError::NoHardwareEncoder)?;
        let context =
            unsafe { device.GetImmediateContext() }.map_err(|_| EncError::NoHardwareEncoder)?;

        // D3D11 的视频接口通过 QueryInterface 取（windows crate 的 `Interface::cast`）。
        let video_device: Option<ID3D11VideoDevice> = device.cast().ok();
        let video_context: Option<ID3D11VideoContext> = context.cast().ok();

        if desc.Format == DXGI_FORMAT_NV12 {
            // 已经是 NV12，不需要 VideoProcessor。
            return Ok(Self {
                _device: device,
                _context: context,
                enumerator: None,
                processor: None,
                output_view: None,
                video_device,
                video_context,
                nv12: src.clone(),
                width: desc.Width,
                height: desc.Height,
                src_format: desc.Format,
            });
        }

        let video_device = video_device.ok_or(EncError::UnsupportedParam {
            what: "D3D11 VideoDevice 接口不可用(QueryInterface 失败)".to_string(),
        })?;
        let video_context = video_context.ok_or(EncError::UnsupportedParam {
            what: "D3D11 VideoContext 接口不可用(QueryInterface 失败)".to_string(),
        })?;

        let nv12 = create_nv12_texture(&device, desc.Width, desc.Height)?;

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
        let enumerator =
            unsafe { video_device.CreateVideoProcessorEnumerator(&content) }.map_err(|_| {
                EncError::UnsupportedParam {
                    what: format!("D3D11 VideoProcessor 不支持源格式 {:?}", desc.Format),
                }
            })?;
        let processor =
            unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }.map_err(|_| {
                EncError::UnsupportedParam {
                    what: "无法创建 D3D11 VideoProcessor".to_string(),
                }
            })?;

        let output_view = create_output_view(&video_device, &enumerator, &nv12, src, desc.Format)?;

        Ok(Self {
            _device: device,
            _context: context,
            enumerator: Some(enumerator),
            processor: Some(processor),
            output_view: Some(output_view),
            video_device: Some(video_device),
            video_context: Some(video_context),
            nv12,
            width: desc.Width,
            height: desc.Height,
            src_format: desc.Format,
        })
    }

    /// 把源纹理转换成 NV12，返回转换结果。
    ///
    /// 源纹理格式与建立时一致时走增量路径；格式变了必须重建 VideoProcessor
    /// （D3D11 的输入视图与枚举器绑定）。
    #[allow(
        unsafe_code,
        reason = "VideoProcessorBlt 与输入视图创建都是 unsafe 的 D3D11 调用"
    )]
    pub fn convert(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D, EncError> {
        // §3.13「颜色转换」阶段的计时在**门面** [`RgbToNv12::convert`] 里，
        // 两条后端共用同一个埋点。
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut desc) };

        if desc.Format == DXGI_FORMAT_NV12 {
            return Ok(src.clone());
        }
        if desc.Format != self.src_format || desc.Width != self.width || desc.Height != self.height
        {
            // 源格式或尺寸变了：重建整个转换器。
            let fresh = Self::new(src)?;
            *self = fresh;
            return Ok(self.nv12.clone());
        }

        let video_device = self
            .video_device
            .as_ref()
            .ok_or(EncError::UnsupportedParam {
                what: "D3D11 VideoDevice 不可用".to_string(),
            })?;
        let video_context = self
            .video_context
            .as_ref()
            .ok_or(EncError::UnsupportedParam {
                what: "D3D11 VideoContext 不可用".to_string(),
            })?;
        let enumerator = self.enumerator.as_ref().ok_or(EncError::UnsupportedParam {
            what: "VideoProcessor 未建立".to_string(),
        })?;
        let processor = self.processor.as_ref().ok_or(EncError::UnsupportedParam {
            what: "VideoProcessor 未建立".to_string(),
        })?;
        let output_view = self
            .output_view
            .as_ref()
            .ok_or(EncError::UnsupportedParam {
                what: "VideoProcessor 输出视图未建立".to_string(),
            })?;

        let input_view = create_input_view(video_device, enumerator, src, desc.Format)?;
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: BOOL(1),
            OutputIndex: 0,
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ..Default::default()
        };

        unsafe {
            video_context.VideoProcessorBlt(
                processor,
                output_view,
                0,
                std::slice::from_ref(&stream),
            )
        }
        .map_err(|_| EncError::UnsupportedParam {
            what: "D3D11 VideoProcessorBlt 失败".to_string(),
        })?;

        Ok(self.nv12.clone())
    }

    /// NV12 目标纹理的裸指针（`ID3D11Texture2D *`）。
    ///
    /// ffmpeg 的 `AV_PIX_FMT_D3D11` 帧约定：`data[0]` 放 `ID3D11Texture2D *`。
    pub fn nv12_ptr(&self) -> *mut c_void {
        self.nv12.as_raw()
    }
}

/// GPU 上的 RGB→NV12 转换器（对外门面）。
///
/// §3.6：「GPU 上 RGB→NV12（**D3D11 VideoProcessor 或 compute shader**）」——
/// 两条路都被文档允许，本门面按下面的顺序挑选：
///
/// 1. **源纹理已经是 NV12** → 直接透传（无需转换）；
/// 2. **D3D11 VideoProcessor** → 驱动支持时优先用它（硬件缩放 + 色彩转换，
///    一步到位）；
/// 3. **compute shader** → VideoProcessor 建不出来时回退
///    （本机 RTX 3070 / 驱动 596.49 上 VideoProcessor 的
///    `CreateVideoProcessorInputView` 恒返回 `E_INVALIDARG`，
///    96 组参数变体全灭，见 `examples/color_vp_probe.rs`）。
///
/// 两条后端产出**同样的 NV12 像素**（BT.709 + limited range），
/// 因此回退不改变画质口径。
pub struct RgbToNv12 {
    inner: Inner,
}

/// 门面内部的三种状态。
enum Inner {
    /// 源纹理本来就是 NV12：透传。
    Passthrough(ID3D11Texture2D),
    /// D3D11 VideoProcessor 后端。
    VideoProcessor(VideoProcessorPath),
    /// compute shader 后端。
    Compute(crate::color_compute::ComputeConverter),
}

impl std::fmt::Debug for RgbToNv12 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgbToNv12")
            .field("backend", &self.backend())
            .finish()
    }
}

impl RgbToNv12 {
    /// 针对给定的源纹理建立转换器。
    ///
    /// VideoProcessor 建不起来时会**自动回退**到 compute shader；
    /// 两条路都不行才报错（`E6003`）。
    pub fn new(src: &ID3D11Texture2D) -> Result<Self, EncError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        #[allow(
            unsafe_code,
            reason = "GetDesc 是 unsafe 的 D3D11 COM 调用（本函数只读源纹理的描述）"
        )]
        unsafe {
            src.GetDesc(&mut desc)
        };

        if desc.Format == DXGI_FORMAT_NV12 {
            // 已经是 NV12，不需要转换。
            return Ok(Self {
                inner: Inner::Passthrough(src.clone()),
            });
        }

        match VideoProcessorPath::new(src) {
            Ok(path) => Ok(Self {
                inner: Inner::VideoProcessor(path),
            }),
            Err(vp_err) => match crate::color_compute::ComputeConverter::new(src) {
                Ok(compute) => {
                    tracing::info!(
                        "D3D11 VideoProcessor 不可用({vp_err}),改用 compute shader 做 RGB→NV12(§3.6 允许的两条路之一)"
                    );
                    Ok(Self {
                        inner: Inner::Compute(compute),
                    })
                }
                Err(cs_err) => Err(EncError::UnsupportedParam {
                    what: format!(
                        "RGB→NV12 两条路都不可用: VideoProcessor={vp_err}; compute shader={cs_err}"
                    ),
                }),
            },
        }
    }

    /// 当前实际使用的后端名（诊断用）。
    pub fn backend(&self) -> &'static str {
        match &self.inner {
            Inner::Passthrough(_) => "passthrough",
            Inner::VideoProcessor(_) => "video-processor",
            Inner::Compute(_) => "compute-shader",
        }
    }

    /// 把源纹理转换成 NV12，返回转换结果。
    ///
    /// 源纹理格式/尺寸变了会自动重建底层转换器。
    pub fn convert(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D, EncError> {
        // §3.13「颜色转换」阶段：GPU 上的 RGB→NV12。
        // 源纹理本来就是 NV12 时这里是一次**透传**（直接返回源纹理），
        // 记下来的耗时同样是真实测量值（调用本身的耗时），不填 0 冒充。
        // `perf_span!` 在 perf-stats 关闭时展开成 unit —— 但**必须**保留这个绑定：
        // 打开时它是一个 span 守卫，绑定与否决定了测量区间是否覆盖整段代码。
        #[allow(
            clippy::let_unit_value,
            reason = "perf-stats 关闭时宏展开成 unit，但守卫必须绑定"
        )]
        let _color_span = perf_span!("color_convert");
        match &mut self.inner {
            Inner::Passthrough(nv12) => Ok(nv12.clone()),
            Inner::VideoProcessor(path) => path.convert(src),
            Inner::Compute(compute) => compute.convert(src),
        }
    }

    /// NV12 目标纹理的裸指针（`ID3D11Texture2D *`）。
    ///
    /// ffmpeg 的 `AV_PIX_FMT_D3D11` 帧约定：`data[0]` 放 `ID3D11Texture2D *`。
    pub fn nv12_ptr(&self) -> *mut c_void {
        match &self.inner {
            Inner::Passthrough(tex) => tex.as_raw(),
            Inner::VideoProcessor(path) => path.nv12_ptr(),
            Inner::Compute(compute) => compute.nv12_ptr(),
        }
    }
}

/// 创建一张 NV12 纹理。
#[allow(unsafe_code, reason = "CreateTexture2D 是 unsafe 的 D3D11 COM 调用")]
fn create_nv12_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, EncError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        // VideoProcessor 的输出面必须带 D3D11_BIND_VIDEO_ENCODER；
        // 实测普通 NV12 纹理上 `D3D11_BIND_SHADER_RESOURCE|D3D11_BIND_VIDEO_ENCODER`
        // 会被 CreateTexture2D 拒（E_INVALIDARG），所以这里只用 VIDEO_ENCODER。
        BindFlags: D3D11_BIND_VIDEO_ENCODER.0 as u32,
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
            .map_err(|_| EncError::NoHardwareEncoder)?;
    }
    texture.ok_or(EncError::NoHardwareEncoder)
}

/// 创建 VideoProcessor 的输入视图。
#[allow(
    unsafe_code,
    reason = "CreateVideoProcessorInputView 是 unsafe 的 D3D11 COM 调用"
)]
fn create_input_view(
    video_device: &ID3D11VideoDevice,
    enumerator: &ID3D11VideoProcessorEnumerator,
    src: &ID3D11Texture2D,
    format: DXGI_FORMAT,
) -> Result<ID3D11VideoProcessorInputView, EncError> {
    let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: format.0 as u32,
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
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("无法创建 D3D11 VideoProcessor 输入视图: {e:?}"),
            })?;
    }
    view.ok_or(EncError::UnsupportedParam {
        what: "D3D11 VideoProcessor 输入视图为空".to_string(),
    })
}

/// 创建 VideoProcessor 的输出视图。
///
/// 有些驱动要求**输入视图先于输出视图**存在（输出视图创建时会校验整条管线），
/// 所以这里先把输入视图建出来再返回。
#[allow(
    unsafe_code,
    reason = "CreateVideoProcessorOutputView 是 unsafe 的 D3D11 COM 调用"
)]
fn create_output_view(
    video_device: &ID3D11VideoDevice,
    enumerator: &ID3D11VideoProcessorEnumerator,
    dst: &ID3D11Texture2D,
    src: &ID3D11Texture2D,
    src_format: DXGI_FORMAT,
) -> Result<ID3D11VideoProcessorOutputView, EncError> {
    // 先建输入视图。
    let _input_view = create_input_view(video_device, enumerator, src, src_format)?;
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
                dst,
                enumerator,
                &desc,
                Some(&mut view as *mut Option<ID3D11VideoProcessorOutputView>),
            )
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("无法创建 D3D11 VideoProcessor 输出视图: {e:?}"),
            })?;
    }
    view.ok_or(EncError::UnsupportedParam {
        what: "D3D11 VideoProcessor 输出视图为空".to_string(),
    })
}

/// NV12 纹理的 CPU 回读结果（Y 平面 + UV 平面）。
#[derive(Debug, Clone)]
pub struct Nv12Planes {
    /// Y 平面数据。
    pub y: Vec<u8>,
    /// UV 平面数据（NV12 的交织 UV）。
    pub uv: Vec<u8>,
    /// 每行字节数。
    pub linesize: usize,
}

/// 把一张 NV12 纹理回读到内存。
///
/// 编码器不支持 `AV_PIX_FMT_D3D11` 时用这条路径：GPU 仍然完成颜色转换，
/// 只是最后多一次 CPU 拷贝。
#[allow(
    unsafe_code,
    reason = "需要创建 staging 纹理并调用 D3D11 的 Map/Unmap（unsafe）"
)]
pub fn read_back_nv12(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    nv12: &ID3D11Texture2D,
) -> Result<Nv12Planes, EncError> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { nv12.GetDesc(&mut desc) };

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: desc.Width,
        Height: desc.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(
                &staging_desc,
                None,
                Some(&mut staging as *mut Option<ID3D11Texture2D>),
            )
            .map_err(|_| EncError::NoHardwareEncoder)?;
    }
    let staging = staging.ok_or(EncError::NoHardwareEncoder)?;

    unsafe { context.CopyResource(&staging, nv12) };

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(
                &staging,
                0,
                D3D11_MAP_READ,
                0,
                Some(&mut mapped as *mut D3D11_MAPPED_SUBRESOURCE),
            )
            .map_err(|_| EncError::NoHardwareEncoder)?;
    }

    let row_pitch = mapped.RowPitch as usize;
    let height = desc.Height as usize;
    let base = mapped.pData as *const u8;
    let mut y = vec![0u8; row_pitch * height];
    let mut uv = vec![0u8; row_pitch * height / 2];
    if !base.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(base, y.as_mut_ptr(), y.len());
            std::ptr::copy_nonoverlapping(base.add(row_pitch * height), uv.as_mut_ptr(), uv.len());
        }
    }

    unsafe { context.Unmap(&staging, 0) };

    Ok(Nv12Planes {
        y,
        uv,
        linesize: row_pitch,
    })
}
