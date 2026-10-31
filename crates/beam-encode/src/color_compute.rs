//! GPU 上的 RGB→NV12 颜色转换 —— **compute shader 路径**（§3.6）。
//!
//! ## 为什么要有第二条路径
//!
//! §3.6 写的是「GPU 上 RGB→NV12（**D3D11 VideoProcessor 或 compute shader**）」——
//! 两条路都被文档允许。
//!
//! 本机（RTX 3070 / 驱动 596.49）上 **VideoProcessor 这条路已经走不通**：
//! `CreateVideoProcessorInputView` 恒返回 `E_INVALIDARG(0x80070057)`。
//! 为排除「是参数写错了」，做过一次 **96 组变体矩阵**（FourCC 取 DXGI 枚举值
//! 还是真 FOURCC × `ContentDesc.Usage` 四种 × 源纹理绑定两种 ×
//! 目标纹理绑定两种，见 `examples/color_vp_probe.rs`）——
//! **一组都没建出来**。所以这是驱动侧的能力缺失，不是参数问题。
//!
//! compute shader 只用到 D3D11 的通用计算能力（FL 11.0 起所有 GPU 都有），
//! 因此作为**回退路径**实现。
//!
//! ## 转换规则
//!
//! - 色彩矩阵：**BT.709**（HD 视频的通用矩阵）；
//! - 量化范围：**limited range**（Y ∈ [16,235]，C ∈ [16,240]），
//!   与 H.264/HEVC 的默认解释一致，客户端解码后按 limited 展开回 RGB；
//! - 输入不做线性化（gamma 域直接做矩阵变换）—— 与主流实时串流实现一致，
//!   也避免一次额外的 GPU 往返；
//! - NV12 的 UV 是 2×2 下采样的交织：**宽/2 × 高/2** 个 R8G8 采样，
//!   每 2×2 块取左上角像素。
//!
//! ## 数据流（**零拷贝**）
//!
//! ```text
//! 源纹理(BGRA8/RGBA8) ──SRV──> compute shader ──UAV──> NV12 纹理的 Y 平面 (R8)
//!                                          └────UAV──> NV12 纹理的 UV 平面 (R8G8)
//! ```
//!
//! NV12 纹理带 `D3D11_BIND_VIDEO_ENCODER | D3D11_BIND_UNORDERED_ACCESS`：
//! - `VIDEO_ENCODER` ⇒ NVENC 能直接吃这张纹理（不用回读到内存）；
//! - `UNORDERED_ACCESS` ⇒ compute shader 能**直接写进它的两个平面**。
//!
//! ## 为什么不能「先写单平面纹理、再拷进 NV12」
//!
//! 实测（RTX 3070）：带 `D3D11_BIND_VIDEO_ENCODER` 的纹理**不能参与拷贝**
//! （拷进去的内容读出来全 0）。所以只能让 compute shader **直写** NV12 的平面，
//! 这也顺带省掉一次全帧拷贝。
//!
//! ## 验证时为什么还要一个「不带 VIDEO_ENCODER」的变体
//!
//! 同理，带 `VIDEO_ENCODER` 的纹理也不能被**拷出来**做校验。
//! 因此 [`ComputeConverter::new_readback`] 建一张不带该标志的 NV12 纹理，
//! 供 [`read_back_nv12_compute`] 回读比对 —— 着色器写入路径与生产完全相同，
//! 只有绑定标志不同。

use std::ffi::c_void;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude, D3D_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext, ID3D11Query,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11UnorderedAccessView,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_UNORDERED_ACCESS, D3D11_BIND_VIDEO_ENCODER,
    D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_FLOAT32_MAX,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11_SAMPLER_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0,
    D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_SRV, D3D11_TEX2D_UAV, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_UAV_DIMENSION_TEXTURE2D, D3D11_UNORDERED_ACCESS_VIEW_DESC,
    D3D11_UNORDERED_ACCESS_VIEW_DESC_0, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC,
};

use crate::error::EncError;

/// 转换用的 HLSL 源码（运行时用 `D3DCompile` 编译）。
///
/// 用 `Texture2D::Load` 而不是 `Sample` —— 抓屏纹理是 1:1 采样，
/// 不需要过滤器，也就没有采样器与纹理格式（整数格式不支持过滤）的坑。
const HLSL_SOURCE: &str = r#"
Texture2D<float4>     src    : register(t0);
RWTexture2D<float>    y_out  : register(u0);
RWTexture2D<float2>   uv_out : register(u1);
RWTexture2D<float2>   uv_shadow : register(u2);

cbuffer Params : register(b0)
{
    uint width;
    uint height;
    uint src_is_bgra;   // 1 = BGRA（DXGI 的 B8G8R8A8），0 = RGBA
    uint pad0;
    uint pad1;
    uint pad2;
    uint pad3;
    uint pad4;
};

// D3D11 的入口点名要通过 D3DCompile 的 pentrypoint 指定（这里用 CSMain）。
[numthreads(8, 8, 1)]
void CSMain(uint3 dtid : SV_DispatchThreadID)
{
    if (dtid.x >= width || dtid.y >= height) {
        return;
    }
    uint2 pos = dtid.xy;
    float4 c = src.Load(int3(pos.x, pos.y, 0));

    // 源纹理的字节序：BGRA8 存的是 (B, G, R, A)，RGBA8 存的是 (R, G, B, A)。
    float r, g, b;
    if (src_is_bgra != 0u) {
        b = c.x; g = c.y; r = c.z;
    } else {
        r = c.x; g = c.y; b = c.z;
    }

    // BT.709 矩阵（直接作用在 gamma 域的值上）。
    float Y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    float U = (b - Y) / 1.8556;
    float V = (r - Y) / 1.5748;

    // limited range：Y ∈ [16,235]，C ∈ [16,240]。
    y_out[pos] = 16.0 + 219.0 * Y;

    // NV12 的 UV 平面是 **宽/2 × 高/2** 个 R8G8 采样（每行 width 字节）。
    // 因此只在 2×2 块的左上角写入，且坐标要各除以 2。
    if ((pos.y & 1u) == 0u && (pos.x & 1u) == 0u) {
        uint2 uvpos = uint2(pos.x >> 1, pos.y >> 1);
        float2 chroma = float2(128.0 + 112.0 * U, 128.0 + 112.0 * V);
        uv_out[uvpos] = chroma;
        // 影子平面：同一份值再写一份到普通 R8G8 纹理。
        // NV12 的平面 1 在本机驱动上**读不回来**（staging 平面 1 报 E_INVALIDARG、
        // 平面拷贝读出来全 0），所以用影子平面把着色器的色度计算钉死。
        uv_shadow[uvpos] = chroma;
    }
}
"#;

/// `D3DCompile` 的目标（compute shader 5.0）。
const COMPILE_TARGET_CS_5_0: &str = "cs_5_0";

/// `D3D_COMPILE_OPTIMIZATION_LEVEL3`（MS 的定义是 `1 << 15`）。
const COMPILE_OPTIMIZATION_LEVEL3: u32 = 1 << 15;

/// 常量缓冲的参数布局（与 HLSL 的 `cbuffer` 一致）。
///
/// HLSL 的常量缓冲大小必须是 **16 字节的整数倍**，因此这里补到 32 字节。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Params {
    width: u32,
    height: u32,
    src_is_bgra: u32,
    pad: [u32; 5],
}

/// compute shader 版本的 RGB→NV12 转换器。
pub struct ComputeConverter {
    /// 设备（保活：所有纹理与视图都挂在它上面）。
    device: ID3D11Device,
    /// 立即上下文。
    context: ID3D11DeviceContext,
    /// 编译出来的计算着色器。
    shader: ID3D11ComputeShader,
    /// Y 平面（R8_UNORM）的 UAV，直接指向 NV12 纹理的平面 0。
    y_uav: ID3D11UnorderedAccessView,
    /// UV 平面（R8G8_UNORM）的 UAV，直接指向 NV12 纹理的平面 1。
    uv_uav: ID3D11UnorderedAccessView,
    /// **影子平面**：同一份色度值再写一份到普通 R8G8 纹理。
    ///
    /// 存在的理由：NV12 的平面 1 在本机驱动上**读不回来**
    /// （staging 平面 1 映射报 E_INVALIDARG、平面拷贝读出来全 0），
    /// 于是「着色器算出来的色度对不对」没法直接验证。
    /// 影子平面是普通单平面纹理，`CopyResource` + `Map` 都能用，
    /// 因此可以把色度计算**逐字节**钉死。
    uv_shadow: ID3D11Texture2D,
    /// 影子平面的 UAV（着色器的 u2）。
    uv_shadow_uav: ID3D11UnorderedAccessView,
    /// 采样器（`Load` 用不到，但 SRV 的绑定仍需要一个）。
    sampler: ID3D11SamplerState,
    /// NV12 目标纹理（带 `VIDEO_ENCODER`，可直接喂 NVENC）。
    nv12: ID3D11Texture2D,
    /// 目标宽（偶数）。
    width: u32,
    /// 目标高（偶数）。
    height: u32,
    /// 建立时的源格式。
    src_format: DXGI_FORMAT,
}

impl std::fmt::Debug for ComputeConverter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputeConverter")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("src_format", &self.src_format)
            .finish()
    }
}

impl ComputeConverter {
    /// 针对给定的源纹理建立转换器（**生产路径**）。
    ///
    /// NV12 目标纹理带 `VIDEO_ENCODER | UNORDERED_ACCESS`：
    /// 前者让 NVENC 直接吃它，后者让 compute shader 直写它的两个平面。
    ///
    /// 源纹理必须是 `BGRA8` 或 `RGBA8`（其它格式直接报错，不静默走偏）。
    pub fn new(src: &ID3D11Texture2D) -> Result<Self, EncError> {
        Self::build(src, true)
    }

    /// 建一个**不带 `VIDEO_ENCODER`** 的转换器（仅供校验 / 诊断）。
    ///
    /// 带 `VIDEO_ENCODER` 的纹理不能被拷出来（实测），因此要回读比对就得用
    /// 这个变体。着色器写入路径与生产完全相同，只有绑定标志不同。
    pub fn new_readback(src: &ID3D11Texture2D) -> Result<Self, EncError> {
        Self::build(src, false)
    }

    /// 两种构造方式的共同实现。
    #[allow(
        unsafe_code,
        reason = "D3D11 的纹理/视图/着色器创建与 dispatch 都是 unsafe 的 COM 调用"
    )]
    fn build(src: &ID3D11Texture2D, video_encoder: bool) -> Result<Self, EncError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut desc) };

        let is_bgra = match desc.Format {
            DXGI_FORMAT_B8G8R8A8_UNORM => 1u32,
            DXGI_FORMAT_R8G8B8A8_UNORM => 0u32,
            other => {
                return Err(EncError::UnsupportedParam {
                    what: format!("compute shader 颜色转换只支持 BGRA8 / RGBA8,收到 {other:?}"),
                })
            }
        };
        let _ = is_bgra;

        // NV12 要求宽高为偶数；抓屏分辨率本来就是偶数，这里再兜一层。
        let width = desc.Width & !1;
        let height = desc.Height & !1;
        if width == 0 || height == 0 {
            return Err(EncError::UnsupportedParam {
                what: "源纹理的宽高为 0 或奇数,无法转成 NV12".to_string(),
            });
        }

        let device = unsafe { src.GetDevice() }.map_err(|_| EncError::NoHardwareEncoder)?;
        let context =
            unsafe { device.GetImmediateContext() }.map_err(|_| EncError::NoHardwareEncoder)?;

        let shader = compile_shader(&device)?;
        let sampler = create_sampler(&device)?;

        let nv12 = create_nv12_texture(&device, width, height, video_encoder)?;
        // 直写 NV12 的两个平面：平面 0 是 R8_UNORM，平面 1 是 R8G8_UNORM。
        let y_uav = create_plane_uav(&device, &nv12, DXGI_FORMAT_R8_UNORM)?;
        let uv_uav = create_plane_uav(&device, &nv12, DXGI_FORMAT_R8G8_UNORM)?;
        // 影子平面：普通 R8G8 纹理，可读可校验。
        let uv_shadow =
            create_plain_texture(&device, width / 2, height / 2, DXGI_FORMAT_R8G8_UNORM)?;
        let uv_shadow_uav = create_plain_uav(&device, &uv_shadow)?;

        Ok(Self {
            device,
            context,
            shader,
            y_uav,
            uv_uav,
            uv_shadow,
            uv_shadow_uav,
            sampler,
            nv12,
            width,
            height,
            src_format: desc.Format,
        })
    }

    /// 当前使用的后端名（诊断用）。
    pub fn backend(&self) -> &'static str {
        "compute-shader"
    }

    /// 读回**影子平面**（着色器写出的色度值，普通 R8G8 纹理）。
    ///
    /// 校验用：NV12 的平面 1 在本机驱动上读不回来，色度计算靠它来钉。
    pub fn read_back_uv_shadow(
        &self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Vec<u8>, EncError> {
        read_plain_texture(
            device,
            context,
            &self.uv_shadow,
            DXGI_FORMAT_R8G8_UNORM,
            self.width / 2,
            self.height / 2,
        )
    }

    /// 目标 NV12 纹理。
    pub fn nv12(&self) -> &ID3D11Texture2D {
        &self.nv12
    }

    /// **阻塞**等待 GPU 把已提交的命令跑完（诊断与计时用）。
    ///
    /// GPU 的命令是异步提交的：不等待的话，测到的只是「CPU 把命令塞进队列」
    /// 的时间，而不是真正的转换耗时。这里用 D3D11 的事件查询做一次同步。
    #[allow(unsafe_code, reason = "事件查询与 GetData 都是 unsafe 的 D3D11 调用")]
    pub fn wait_idle(&self) -> Result<(), EncError> {
        let mut query: Option<ID3D11Query> = None;
        unsafe {
            let query_desc = D3D11_QUERY_DESC {
                Query: D3D11_QUERY_EVENT,
                MiscFlags: 0,
            };
            self.device
                .CreateQuery(&query_desc, Some(&mut query as *mut Option<ID3D11Query>))
                .map_err(|e| EncError::UnsupportedParam {
                    what: format!("创建事件查询失败: {e:?}"),
                })?;
            let query = query.ok_or(EncError::UnsupportedParam {
                what: "事件查询为空".to_string(),
            })?;
            self.context.Begin(&query);
            self.context.End(&query);
            // `GetData` 在查询还没完成时返回 **S_FALSE**（也是「成功」），
            // 所以必须循环到返回值为 TRUE（1）为止，否则等不到 GPU。
            let mut done = 0u32;
            let mut waited = false;
            for _ in 0..10_000 {
                self.context
                    .GetData(&query, Some(&mut done as *mut u32 as *mut c_void), 4, 0)
                    .map_err(|e| EncError::UnsupportedParam {
                        what: format!("等待 GPU 完成失败: {e:?}"),
                    })?;
                if done != 0 {
                    waited = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
            if !waited {
                return Err(EncError::UnsupportedParam {
                    what: "等待 GPU 完成超时".to_string(),
                });
            }
        }
        Ok(())
    }

    /// 把源纹理转换成 NV12。
    ///
    /// 源纹理的格式/尺寸与建立时不一致时**重建**转换器
    /// （SRV 与常量缓冲都绑定了尺寸）。
    #[allow(
        unsafe_code,
        reason = "需要创建 SRV、绑定管线、派发 compute shader（都是 unsafe 调用）"
    )]
    pub fn convert(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D, EncError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut desc) };
        if desc.Format != self.src_format
            || (desc.Width & !1) != self.width
            || (desc.Height & !1) != self.height
        {
            let fresh = Self::build(src, false).or_else(|_| Self::build(src, true))?;
            *self = fresh;
            return Ok(self.nv12.clone());
        }

        // 源纹理的 SRV。
        let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: desc.Format,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                },
            },
        };
        let mut srv: Option<ID3D11ShaderResourceView> = None;
        unsafe {
            self.device
                .CreateShaderResourceView(src, Some(&srv_desc), Some(&mut srv))
                .map_err(|e| EncError::UnsupportedParam {
                    what: format!("创建源纹理 SRV 失败: {e:?}"),
                })?;
        }
        let srv = srv.ok_or(EncError::UnsupportedParam {
            what: "源纹理 SRV 为空".to_string(),
        })?;

        // 常量缓冲（建在**设备**上，不是上下文上）。
        let params = Params {
            width: self.width,
            height: self.height,
            src_is_bgra: match self.src_format {
                DXGI_FORMAT_B8G8R8A8_UNORM => 1,
                _ => 0,
            },
            pad: [0; 5],
        };
        let mut constant = [0u8; 32];
        constant[0..4].copy_from_slice(&params.width.to_le_bytes());
        constant[4..8].copy_from_slice(&params.height.to_le_bytes());
        constant[8..12].copy_from_slice(&params.src_is_bgra.to_le_bytes());
        let mut cbuffer: Option<ID3D11Buffer> = None;
        unsafe {
            let buf_desc = D3D11_BUFFER_DESC {
                ByteWidth: constant.len() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let initial = D3D11_SUBRESOURCE_DATA {
                pSysMem: constant.as_ptr() as *const c_void,
                SysMemPitch: 0,
                SysMemSlicePitch: 0,
            };
            self.device
                .CreateBuffer(
                    &buf_desc,
                    Some(&initial),
                    Some(&mut cbuffer as *mut Option<ID3D11Buffer>),
                )
                .map_err(|e| EncError::UnsupportedParam {
                    what: format!("创建常量缓冲失败: {e:?}"),
                })?;
        }
        let cbuffer = cbuffer.ok_or(EncError::UnsupportedParam {
            what: "常量缓冲为空".to_string(),
        })?;

        // 绑定管线并派发。
        unsafe {
            let srvs: [Option<ID3D11ShaderResourceView>; 1] = [Some(srv)];
            let samplers: [Option<ID3D11SamplerState>; 1] = [Some(self.sampler.clone())];
            let cbuffers: [Option<ID3D11Buffer>; 1] = [Some(cbuffer)];
            let y_uavs: [Option<ID3D11UnorderedAccessView>; 1] = [Some(self.y_uav.clone())];
            let uv_uavs: [Option<ID3D11UnorderedAccessView>; 1] = [Some(self.uv_uav.clone())];
            let shadow_uavs: [Option<ID3D11UnorderedAccessView>; 1] =
                [Some(self.uv_shadow_uav.clone())];

            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context.CSSetSamplers(0, Some(&samplers));
            self.context.CSSetConstantBuffers(0, Some(&cbuffers));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(y_uavs.as_ptr()), None);
            self.context
                .CSSetUnorderedAccessViews(1, 1, Some(uv_uavs.as_ptr()), None);
            self.context
                .CSSetUnorderedAccessViews(2, 1, Some(shadow_uavs.as_ptr()), None);
            self.context.CSSetShader(&self.shader, None);
            let groups_x = self.width.div_ceil(8);
            let groups_y = self.height.div_ceil(8);
            self.context.Dispatch(groups_x, groups_y, 1);
            // 解绑 UAV（否则后续读同一资源会被 D3D11 拒绝）。
            self.context.CSSetUnorderedAccessViews(0, 0, None, None);
            self.context.CSSetUnorderedAccessViews(1, 0, None, None);
            self.context.CSSetUnorderedAccessViews(2, 0, None, None);
            self.context.CSSetShader(None, None);
            self.context.Flush();
        }

        Ok(self.nv12.clone())
    }

    /// 目标 NV12 纹理的裸指针（ffmpeg `AV_PIX_FMT_D3D11` 帧约定）。
    pub fn nv12_ptr(&self) -> *mut c_void {
        self.nv12.as_raw()
    }
}

/// 编译内嵌的 HLSL。
#[allow(unsafe_code, reason = "D3DCompile 是 unsafe 的 FFI 调用")]
fn compile_shader(device: &ID3D11Device) -> Result<ID3D11ComputeShader, EncError> {
    let src = HLSL_SOURCE.as_bytes();
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // `D3DCompile` 的字符串参数要 NUL 结尾，用 `CString` 兜住。
    let name = std::ffi::CString::new("beam_color.hlsl").expect("常量字符串");
    let entry = std::ffi::CString::new("CSMain").expect("常量字符串");
    let target = std::ffi::CString::new(COMPILE_TARGET_CS_5_0).expect("常量字符串");
    let compiled = unsafe {
        D3DCompile(
            src.as_ptr() as *const c_void,
            src.len(),
            windows::core::PSTR(name.as_ptr() as *mut u8),
            None,
            None::<&ID3DInclude>,
            windows::core::PSTR(entry.as_ptr() as *mut u8),
            windows::core::PSTR(target.as_ptr() as *mut u8),
            COMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code as *mut Option<ID3DBlob>,
            Some(&mut errors as *mut Option<ID3DBlob>),
        )
    };
    if let Err(e) = compiled {
        // 把编译器的报错原文带出来（否则只有一个没头没脑的 E_FAIL）。
        let detail = errors
            .as_ref()
            .map(|blob| unsafe {
                let ptr = blob.GetBufferPointer();
                let size = blob.GetBufferSize();
                if ptr.is_null() || size == 0 {
                    String::new()
                } else {
                    let bytes = std::slice::from_raw_parts(ptr as *const u8, size);
                    String::from_utf8_lossy(bytes).to_string()
                }
            })
            .unwrap_or_default();
        return Err(EncError::UnsupportedParam {
            what: format!("计算着色器编译失败: {e:?}: {detail}"),
        });
    }
    let code = code.ok_or(EncError::UnsupportedParam {
        what: "计算着色器编译产物为空".to_string(),
    })?;

    // `CreateComputeShader` 收的是**字节码切片**，不是 blob 本身。
    let bytecode = unsafe {
        let ptr = code.GetBufferPointer();
        let size = code.GetBufferSize();
        if ptr.is_null() || size == 0 {
            return Err(EncError::UnsupportedParam {
                what: "计算着色器字节码为空".to_string(),
            });
        }
        std::slice::from_raw_parts(ptr as *const u8, size)
    };
    let mut shader: Option<ID3D11ComputeShader> = None;
    unsafe {
        device
            .CreateComputeShader(
                bytecode,
                None,
                Some(&mut shader as *mut Option<ID3D11ComputeShader>),
            )
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建计算着色器失败: {e:?}"),
            })?;
    }
    shader.ok_or(EncError::UnsupportedParam {
        what: "计算着色器为空".to_string(),
    })
}

/// 创建一个最简采样器（`Load` 用不到它，但 SRV 绑定需要一个）。
#[allow(unsafe_code, reason = "创建采样器是 unsafe 的 D3D11 COM 调用")]
fn create_sampler(device: &ID3D11Device) -> Result<ID3D11SamplerState, EncError> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        ComparisonFunc: windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER,
        MaxLOD: D3D11_FLOAT32_MAX,
        ..Default::default()
    };
    let mut sampler: Option<ID3D11SamplerState> = None;
    unsafe {
        device
            .CreateSamplerState(&desc, Some(&mut sampler))
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建采样器失败: {e:?}"),
            })?;
    }
    sampler.ok_or(EncError::UnsupportedParam {
        what: "采样器为空".to_string(),
    })
}

/// 创建 NV12 目标纹理。
///
/// - `video_encoder = true`（生产）：带 `VIDEO_ENCODER | UNORDERED_ACCESS`，
///   NVENC 能直接吃它、compute shader 也能直写它的两个平面；
/// - `video_encoder = false`（校验/诊断）：只带 `UNORDERED_ACCESS`，
///   这样纹理才能被**拷出来**做逐字节比对。
///
/// 实测：带 `VIDEO_ENCODER` 的纹理**不能参与拷贝**（拷进去的内容读出来全 0）。
#[allow(unsafe_code, reason = "CreateTexture2D 是 unsafe 的 D3D11 COM 调用")]
fn create_nv12_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    video_encoder: bool,
) -> Result<ID3D11Texture2D, EncError> {
    let mut bind = D3D11_BIND_UNORDERED_ACCESS.0 as u32;
    if video_encoder {
        bind |= D3D11_BIND_VIDEO_ENCODER.0 as u32;
    }
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
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建 NV12 纹理失败: {e:?}"),
            })?;
    }
    texture.ok_or(EncError::UnsupportedParam {
        what: "NV12 纹理为空".to_string(),
    })
}

/// 在 NV12 纹理的某个平面上开 UAV。
///
/// 视图的**格式**决定平面：R8_UNORM ⇒ 平面 0（Y），R8G8_UNORM ⇒ 平面 1（UV）。
#[allow(
    unsafe_code,
    reason = "CreateUnorderedAccessView 是 unsafe 的 D3D11 COM 调用"
)]
fn create_plane_uav(
    device: &ID3D11Device,
    nv12: &ID3D11Texture2D,
    format: DXGI_FORMAT,
) -> Result<ID3D11UnorderedAccessView, EncError> {
    let desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
        },
    };
    let mut view: Option<ID3D11UnorderedAccessView> = None;
    unsafe {
        device
            .CreateUnorderedAccessView(
                nv12,
                Some(&desc),
                Some(&mut view as *mut Option<ID3D11UnorderedAccessView>),
            )
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建 NV12 平面的 UAV 失败(format={format:?}): {e:?}"),
            })?;
    }
    view.ok_or(EncError::UnsupportedParam {
        what: "UAV 为空".to_string(),
    })
}

/// 把 NV12 纹理的 **Y 平面**回读到内存（测试与诊断用）。
///
/// ## 为什么是「拷成单平面再映射」而不是直接映射 NV12
///
/// D3D11 的 `Map` 不接受 box 参数（那是 D3D12 的 `Map1`），
/// 而本机驱动对 **NV12 staging 纹理的 subresource 1（UV 平面）** 直接返回
/// `E_INVALIDARG`。因此这里把两个平面分别拷到
/// **单平面**的 staging 纹理（R8 / R8G8）再映射 —— 格式简单，各驱动都支持。
///
/// 前提：纹理**不能**带 `D3D11_BIND_VIDEO_ENCODER`（带了的拷不出来），
/// 所以校验时用 [`ComputeConverter::new_readback`] 建的那张。
/// 把 NV12 纹理的 **Y 平面**回读到内存（测试与诊断用）。
///
/// ## 为什么只读 Y 平面
///
/// 本机驱动对 **NV12 staging 纹理的 subresource 1（UV 平面）** 直接返回
/// `E_INVALIDARG`，而 NV12 的平面拷贝（`CopySubresourceRegion`）读出来全 0。
/// 因此色度（UV）的校验改用 [`ComputeConverter::read_back_uv_shadow`] ——
/// 同一份着色器写出的普通 R8G8「影子平面」。
///
/// 前提：纹理**不能**带 `D3D11_BIND_VIDEO_ENCODER`（带了的拷不出来），
/// 所以校验时用 [`ComputeConverter::new_readback`] 建的那张。
#[allow(
    unsafe_code,
    reason = "需要创建 staging 纹理并调用 D3D11 的 Map/Unmap（unsafe）"
)]
pub fn read_back_nv12_compute(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    nv12: &ID3D11Texture2D,
) -> Result<Vec<u8>, EncError> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { nv12.GetDesc(&mut desc) };

    // 路线一：整张拷进 NV12 staging，再映射平面 0（Y）。
    // 本机驱动对 NV12 staging 的**平面 1** 直接报 E_INVALIDARG，
    // 所以 Y 走这条路，UV 走路线二。
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
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
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
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建 NV12 staging 纹理失败: {e:?}"),
            })?;
    }
    let staging = staging.ok_or(EncError::UnsupportedParam {
        what: "NV12 staging 纹理为空".to_string(),
    })?;

    let mut y = vec![0u8; desc.Width as usize * desc.Height as usize];
    unsafe {
        context.CopyResource(&staging, nv12);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("映射 NV12 staging 的 Y 平面失败: {e:?}"),
            })?;
        for row in 0..desc.Height as usize {
            let src = mapped.pData as *const u8;
            let dst = y.as_mut_ptr().add(row * desc.Width as usize);
            std::ptr::copy_nonoverlapping(
                src.add(row * mapped.RowPitch as usize),
                dst,
                desc.Width as usize,
            );
        }
        context.Unmap(&staging, 0);
    }

    Ok(y)
}

/// 创建一张普通纹理（可作 UAV 目标）。
#[allow(unsafe_code, reason = "CreateTexture2D 是 unsafe 的 D3D11 COM 调用")]
fn create_plain_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D, EncError> {
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
        BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
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
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建中间纹理失败(format={format:?}): {e:?}"),
            })?;
    }
    texture.ok_or(EncError::UnsupportedParam {
        what: "中间纹理为空".to_string(),
    })
}

/// 在普通纹理上开一个 UAV。
#[allow(
    unsafe_code,
    reason = "CreateUnorderedAccessView 是 unsafe 的 D3D11 COM 调用"
)]
fn create_plain_uav(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11UnorderedAccessView, EncError> {
    let desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8_UNORM,
        ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
        },
    };
    let mut view: Option<ID3D11UnorderedAccessView> = None;
    unsafe {
        device
            .CreateUnorderedAccessView(
                texture,
                Some(&desc),
                Some(&mut view as *mut Option<ID3D11UnorderedAccessView>),
            )
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建影子平面的 UAV 失败: {e:?}"),
            })?;
    }
    view.ok_or(EncError::UnsupportedParam {
        what: "影子平面的 UAV 为空".to_string(),
    })
}

/// 读回一张**单平面**纹理（整张拷贝 + 映射）。
#[allow(
    unsafe_code,
    reason = "需要创建 staging 纹理并调用 D3D11 的 Map/Unmap（unsafe）"
)]
fn read_plain_texture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, EncError> {
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
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
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("创建 staging 纹理失败(format={format:?}): {e:?}"),
            })?;
    }
    let staging = staging.ok_or(EncError::UnsupportedParam {
        what: "staging 纹理为空".to_string(),
    })?;

    unsafe {
        context.CopyResource(&staging, tex);
        let bpp = bytes_per_pixel(format);
        let mut data = vec![0u8; (width as usize) * (height as usize) * bpp];
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| EncError::UnsupportedParam {
                what: format!("映射单平面纹理失败: {e:?}"),
            })?;
        for row in 0..height as usize {
            let src = mapped.pData as *const u8;
            let dst = data.as_mut_ptr().add(row * width as usize * bpp);
            std::ptr::copy_nonoverlapping(
                src.add(row * mapped.RowPitch as usize),
                dst,
                width as usize * bpp,
            );
        }
        context.Unmap(&staging, 0);
        Ok(data)
    }
}

/// 每个像素的字节数（本处只用到 R8 与 R8G8）。
fn bytes_per_pixel(format: DXGI_FORMAT) -> usize {
    match format {
        DXGI_FORMAT_R8_UNORM => 1,
        _ => 2,
    }
}
