//! D3D11 flip-model 交换链（§3.10）。
//!
//! ## 交换链形态
//!
//! - `DXGI_SWAP_EFFECT_FLIP_DISCARD`：flip-model 交换链（§3.10 要求），
//!   配合 `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING` 允许撕裂；
//! - `BufferCount = 2`：一帧在飞行、一帧在显示；
//! - `SetMaximumFrameLatency(1)`：**flip-model 下会被 DXGI 拒绝**
//!   （实测返回 `DXGI_ERROR_INVALID_CALL`），因此这里只记日志不当作致命错误；
//!   flip-model 的双缓冲本身就给出了「一帧在飞、一帧在显示」的延迟上界。
//!
//! ## flip-model 的一个坑（实测）
//!
//! `IDXGISwapChain::GetBuffer(i)` 在 flip-model 下**只有 `i` 等于
//! `GetCurrentBackBufferIndex()` 时才成功**，否则返回 `DXGI_ERROR_INVALID_CALL`。
//! 所以渲染目标视图必须**按索引懒创建**（见 `back_buffer_view`），
//! 不能在初始化时把两个后备缓冲一次性取出来。
//!
//! ## 可变刷新率（VRR）
//!
//! DXGI 没有单独的「启用 VRR」开关：flip-model 交换链在支持 VRR 的显示器上
//! 天然可以按显示器的实际刷新节奏呈现。本层的做法是：
//!
//! - `vrr = true`：`Present(0, DXGI_PRESENT_ALLOW_TEARING)`——不等垂直刷新，
//!   允许帧在刷新周期中间落地，显示器可以按需拉长/缩短刷新周期；
//! - `vrr = false`：`Present(1, 0)`——严格等垂直刷新（经典 vsync）。
//!
//! 显示器实际跑在什么刷新率，可以用 `IDXGISwapChain3::GetFrameStatistics`
//! 读到的 `PresentRefreshCount` / `SyncRefreshCount` 观测。
//!
//! ## 上屏等待（平均半帧）
//!
//! 帧在刷新周期内均匀到达，因此「等下一个 vblank」的平均等待是**半帧**。
//! 这是设计性质，**没有在本机实测**。

// 本模块需要 unsafe：D3D11 / DXGI 都是裸 COM API——创建设备、创建交换链、
// 创建渲染目标视图、拷贝纹理、呈现都是 unsafe 调用。
#![allow(unsafe_code)]

use windows::core::Interface;
use windows::Win32::Foundation::{FALSE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory2, IDXGISwapChain3, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_FRAME_STATISTICS, DXGI_MWA_NO_ALT_ENTER,
    DXGI_MWA_NO_PRINT_SCREEN, DXGI_MWA_NO_WINDOW_CHANGES, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING,
    DXGI_SWAP_EFFECT, DXGI_SWAP_EFFECT_DISCARD, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::error::RenderError;
use crate::renderer::{FrameStatistics, PresentInfo, RenderConfig, SwapChainFormat};

/// 交换链的缓冲区数量（flip-model 的最小可用值：一帧在飞、一帧在显示）。
const BUFFER_COUNT: u32 = 2;

/// 把配置里的格式枚举翻译成 DXGI 格式。
///
/// `DXGI_FORMAT_420Opaque` 在 windows 绑定里没有对应常量（DXGI 1.3
/// 引入的视频格式），这里按其**文档化的数值** 103 构造。
fn dxgi_format(fmt: SwapChainFormat) -> DXGI_FORMAT {
    DXGI_FORMAT(fmt.dxgi_code())
}

/// 这个格式是不是 4:2:0 视频格式（多平面纹理，建不了渲染目标视图）。
fn is_video_format(fmt: DXGI_FORMAT) -> bool {
    fmt == dxgi_format(SwapChainFormat::Nv12) || fmt == dxgi_format(SwapChainFormat::Opaque420)
}

/// 把「平台指针」借用成 D3D11 纹理引用（**不**接管所有权）。
///
/// beam-proto 的 `PlatformHandle` 按约定承载平台指针（§4 分层说明）：
/// 这里把它解释成 D3D11 纹理指针，并且只**借用**（不做 AddRef/Release）。
///
/// 借用必须绑定在一个**活着**的裸指针变量上（`from_raw_borrowed` 的签名
/// 要求 `&raw`），所以这里用 `'_` 借用调用方作用域里的指针。
fn borrow_texture(raw: &*mut core::ffi::c_void) -> Option<&ID3D11Texture2D> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: 借用调用方持有的 COM 指针，不接管所有权、不做 AddRef/Release。
    unsafe { windows::core::Interface::from_raw_borrowed(raw) }
}

/// D3D11 flip-model 交换链。
#[derive(Debug)]
pub(crate) struct SwapChain {
    /// D3D11 设备（解码侧必须复用同一个设备才能做 GPU 内拷贝）。
    device: ID3D11Device,
    /// 立即上下文。
    context: ID3D11DeviceContext,
    /// flip-model 交换链。
    swapchain: IDXGISwapChain3,
    /// 每个后备缓冲一个渲染目标视图（按需创建，见 `create` 的注释）。
    rtvs: Vec<Option<ID3D11RenderTargetView>>,
    /// 后备缓冲宽。
    width: u32,
    /// 后备缓冲高。
    height: u32,
    /// 后备缓冲格式。
    format: DXGI_FORMAT,
    /// 是否走可变刷新率路径。
    vrr: bool,
    /// 呈现诊断信息（§3.10：真的搬纹理 vs 降级清屏）。
    info: PresentInfo,
    /// VideoProcessor 转换器（4:2:0 → BGRA，§3.10）。
    ///
    /// 首次需要时按当前解码纹理建立；源格式或高度变了要重建。
    converter: Option<crate::vp::VpConverter>,
}

impl SwapChain {
    /// 在给定窗口上创建 flip-model 交换链。
    pub(crate) fn create(hwnd: HWND, cfg: &RenderConfig) -> Result<Self, RenderError> {
        let (device, context) = create_device()?;
        // `CreateSwapChainForHwnd` 在 `IDXGIFactory2` 上（DXGI 1.2 引入）。
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1() }.map_err(|err| {
            tracing::error!(error = %err, "创建 DXGI 工厂失败");
            RenderError::SwapChainFailed {
                reason: "创建 DXGI 工厂失败".to_string(),
            }
        })?;

        let swap_effect: DXGI_SWAP_EFFECT = if cfg.flip_model {
            DXGI_SWAP_EFFECT_FLIP_DISCARD
        } else {
            // 非 flip-model 走传统的丢弃式交换链（VRR 不可用）。
            DXGI_SWAP_EFFECT_DISCARD
        };

        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: cfg.width,
            Height: cfg.height,
            Format: dxgi_format(cfg.format),
            Stereo: FALSE,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: swap_effect,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            // 允许撕裂是 VRR / 低延迟呈现的前提。
            Flags: DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32,
        };

        let swapchain = unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) }
            .map_err(|err| {
                tracing::error!(error = %err, "创建 flip-model 交换链失败");
                RenderError::SwapChainFailed {
                    reason: "创建 flip-model 交换链失败".to_string(),
                }
            })?;

        // 阻止 Alt+Enter / 打印屏等系统动作打断 flip-model。
        let _ = unsafe {
            factory.MakeWindowAssociation(
                hwnd,
                DXGI_MWA_NO_WINDOW_CHANGES | DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_PRINT_SCREEN,
            )
        };

        let swapchain: IDXGISwapChain3 = swapchain.cast().map_err(|err| {
            tracing::error!(error = %err, "获取 IDXGISwapChain3 接口失败");
            RenderError::SwapChainFailed {
                reason: "获取 IDXGISwapChain3 接口失败".to_string(),
            }
        })?;

        // 尽量把在途帧数压到 1（低延迟）。flip-model 交换链可能拒绝这个调用
        // （实测返回 DXGI_ERROR_INVALID_CALL），这不是致命错误：
        // flip-model 的双缓冲本身就给出了「一帧在飞、一帧在显示」的延迟上界。
        if let Err(err) = unsafe { swapchain.SetMaximumFrameLatency(1) } {
            tracing::debug!(
                hr = err.code().0,
                "设置最大帧延迟被拒绝,flip-model 的双缓冲已经提供同等延迟上界"
            );
        }

        // flip-model 交换链的每个后备缓冲一个渲染目标视图。
        // 注意：flip-model 下 `GetBuffer(i)` 只有在 `i == GetCurrentBackBufferIndex()`
        // 时才允许（否则返回 DXGI_ERROR_INVALID_CALL，实测结论），
        // 所以视图必须**按需**创建，不能在初始化时一次性把两个都取出来。
        let rtvs = (0..BUFFER_COUNT).map(|_| None).collect();

        tracing::info!(
            width = cfg.width,
            height = cfg.height,
            flip_model = cfg.flip_model,
            vrr = cfg.vrr,
            "D3D11 flip-model 交换链已创建"
        );

        Ok(Self {
            device,
            context,
            swapchain,
            rtvs,
            width: cfg.width,
            height: cfg.height,
            format: dxgi_format(cfg.format),
            vrr: cfg.vrr,
            info: PresentInfo {
                swapchain_format: dxgi_format(cfg.format).0,
                ..PresentInfo::default()
            },
            converter: None,
        })
    }

    /// D3D11 设备（供解码侧复用同一设备）。
    pub(crate) fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// 呈现诊断信息（§3.10）。
    pub(crate) fn present_info(&self) -> PresentInfo {
        self.info
    }

    /// 呈现一帧。
    ///
    /// `source_handle` 是解码纹理的指针（`DecodedFrame::surface` 的整数值）：
    /// `0` 表示这一拍没有可用的解码数据。
    ///
    /// ## 两条路径
    ///
    /// - **视频格式**（`DXGI_FORMAT_NV12` / `DXGI_FORMAT_420Opaque`）：
    ///   后备缓冲是 4:2:0 多平面纹理，**建不了渲染目标视图** ⇒
    ///   直接把解码纹理 `CopyResource` 进去（零拷贝，§3.10）。
    /// - **BGRA**：后备缓冲可建 RTV，解码纹理格式不同时只能清屏
    ///   （此时画面上不了屏，由 [`PresentInfo`] 如实记录）。
    pub(crate) fn present(&mut self, source_handle: usize) -> Result<(), RenderError> {
        let index = unsafe { self.swapchain.GetCurrentBackBufferIndex() } as usize;
        if is_video_format(self.format) {
            self.present_video(index, source_handle)?;
        } else {
            self.present_bgra(index, source_handle)?;
        }
        self.info.presents += 1;
        self.present_to_screen()
    }

    /// 视频格式路径：零拷贝把解码纹理搬进 4:2:0 后备缓冲。
    ///
    /// 不建 RTV、不清屏：4:2:0 多平面纹理的渲染目标视图不受支持。
    fn present_video(&mut self, index: usize, source_handle: usize) -> Result<(), RenderError> {
        let raw = source_handle as *mut core::ffi::c_void;
        let Some(source) = borrow_texture(&raw) else {
            // 这一拍没有解码数据：保留上一帧（视频交换链不清屏）。
            self.info.cleared_presents += 1;
            return Ok(());
        };
        let (desc, same_device) = self.describe_source(source);
        if desc.Width == self.width
            && desc.Height == self.height
            && desc.Format == self.format
            && same_device
        {
            let buffer: ID3D11Texture2D = unsafe { self.swapchain.GetBuffer(index as u32) }
                .map_err(|err| {
                    tracing::error!(error = %err, "获取后备缓冲失败");
                    RenderError::SwapChainFailed {
                        reason: "获取后备缓冲失败".to_string(),
                    }
                })?;
            // SAFETY: 源与目标同设备、同尺寸、同格式，满足 `CopyResource` 的前置条件。
            unsafe { self.context.CopyResource(&buffer, source) };
            self.info.textured_presents += 1;
        } else {
            tracing::warn!(
                src_width = desc.Width,
                src_height = desc.Height,
                src_format = desc.Format.0,
                dst_width = self.width,
                dst_height = self.height,
                dst_format = self.format.0,
                same_device,
                "解码纹理与视频交换链不匹配,本帧保留上一帧"
            );
            self.info.cleared_presents += 1;
        }
        Ok(())
    }

    /// BGRA 路径：渲染目标视图 + 清屏兜底。
    fn present_bgra(&mut self, index: usize, source_handle: usize) -> Result<(), RenderError> {
        let rtv = self.back_buffer_view(index)?;

        // 拷贝目标不能同时是当前渲染目标，先解绑。
        unsafe {
            self.context.OMSetRenderTargets(
                Some(&[]),
                None::<&windows::Win32::Graphics::Direct3D11::ID3D11DepthStencilView>,
            )
        };

        let mut cleared = true;
        let mut mismatch: Option<String> = None;
        let raw = source_handle as *mut core::ffi::c_void;
        if let Some(source) = borrow_texture(&raw) {
            let (desc, same_device) = self.describe_source(source);
            // D3D11 **不允许跨设备拷贝**：解码纹理必须与交换链在同一个设备上
            // （解码侧请复用 `D3D11Renderer::device`）。这里做一次运行期校验，
            // 免得设备不匹配时静默拷出错误内容。
            if desc.Width == self.width
                && desc.Height == self.height
                && desc.Format == self.format
                && same_device
            {
                let buffer: ID3D11Texture2D = unsafe { self.swapchain.GetBuffer(index as u32) }
                    .map_err(|err| {
                        tracing::error!(error = %err, "获取后备缓冲失败");
                        RenderError::SwapChainFailed {
                            reason: "获取后备缓冲失败".to_string(),
                        }
                    })?;
                // SAFETY: 源与目标同设备、同尺寸、同格式，
                // 且目标当前未被绑定为渲染目标，满足 `CopyResource` 的前置条件。
                unsafe { self.context.CopyResource(&buffer, source) };
                cleared = false;
                self.info.textured_presents += 1;
            } else {
                // §3.10：格式 / 尺寸对不上时，先试 **VideoProcessor 转换**
                // （4:2:0 → BGRA，硬件做色彩空间转换）。
                //
                // 转换成功后拿 BGRA 纹理再走一次 CopyResource —— 那就是真的上屏。
                if let Some(converted) = self.convert_420_to_bgra(source, desc) {
                    // 转换结果要先脱离借用（`self` 还要用），再拷进后备缓冲。
                    let converted = converted.clone();
                    let buffer: ID3D11Texture2D = unsafe { self.swapchain.GetBuffer(index as u32) }
                        .map_err(|err| {
                            tracing::error!(error = %err, "获取后备缓冲失败");
                            RenderError::SwapChainFailed {
                                reason: "获取后备缓冲失败".to_string(),
                            }
                        })?;
                    // SAFETY: 转换结果是 BGRA、与交换链后备缓冲同格式同尺寸同设备。
                    unsafe { self.context.CopyResource(&buffer, &converted) };
                    self.info.textured_presents += 1;
                    self.info.converted_presents += 1;
                    return Ok(());
                }
                tracing::warn!(
                    src_width = desc.Width,
                    src_height = desc.Height,
                    src_format = desc.Format.0,
                    dst_width = self.width,
                    dst_height = self.height,
                    dst_format = self.format.0,
                    same_device,
                    "解码纹理与交换链不匹配,本帧改为清屏"
                );
                // **不静默降级**：早先这里只清屏然后照样走完呈现路径，
                // 不报错 ⇒ 上层以为「在串流」，用户看到的是空白窗口。
                // 现在如实报错（E10015），让上层能告诉用户「画面上不来」。
                mismatch = Some(format!(
                    "解码纹理 {}x{} (格式 0x{:08x}, 同设备={}) 与交换链 {}x{} (格式 0x{:08x}) 不匹配",
                    desc.Width,
                    desc.Height,
                    desc.Format.0,
                    same_device,
                    self.width,
                    self.height,
                    self.format.0,
                ));
            }
        }

        unsafe {
            self.context
                .OMSetRenderTargets(Some(&[Some(rtv.clone())]), None)
        };
        if cleared {
            // SAFETY: `rtv` 是本交换链自己创建的渲染目标视图。
            unsafe {
                self.context
                    .ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0])
            };
            self.info.cleared_presents += 1;
        }
        if let Some(reason) = mismatch {
            // 画面搬不上去 ⇒ 如实报错，不让上层以为「在串流」。
            return Err(RenderError::SourceMismatch { reason });
        }
        Ok(())
    }

    /// 用 **VideoProcessor** 把 4:2:0 解码纹理转换成 BGRA（§3.10）。
    ///
    /// D3D11VA 的原生输出是 `DXGI_FORMAT_420Opaque`，与 BGRA 交换链格式不同
    /// ⇒ 零拷贝不可行（B17 实测）。VideoProcessor 是硬件做的格式转换。
    ///
    /// 返回转换后的 BGRA 纹理（`None` = 转换不可用）。
    #[allow(
        unsafe_code,
        reason = "VideoProcessor 的创建与 blt 都是 unsafe 的 D3D11 COM 调用"
    )]
    fn convert_420_to_bgra(
        &mut self,
        source: &ID3D11Texture2D,
        desc: D3D11_TEXTURE2D_DESC,
    ) -> Option<ID3D11Texture2D> {
        if !crate::vp::is_420_format(desc.Format) {
            return None;
        }
        // 可见高度 = 交换链的高度（纹理自身的高是对齐后的 coded height）。
        // 4:2:0 的 Y 平面行距 = 宽度，解码器把画面写在前面、后面是 padding
        // ⇒ 转换必须按可见高度取，否则底部会多出未初始化的行。
        let visible_height = desc.Height.min(self.height);
        let needs_rebuild = match &self.converter {
            Some(c) => {
                let (w, h) = c.dimensions();
                w != desc.Width || h != visible_height
            }
            None => true,
        };
        if needs_rebuild {
            match crate::vp::VpConverter::new(source, visible_height) {
                Ok(c) => self.converter = Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "VideoProcessor 转换器建立失败,本帧无法上屏");
                    return None;
                }
            }
        }
        let converter = self.converter.as_mut()?;
        match converter.convert(source) {
            Ok(target) => Some(target.clone()),
            Err(e) => {
                tracing::warn!(error = %e, "VideoProcessor 转换失败,本帧无法上屏");
                self.converter = None;
                None
            }
        }
    }

    /// 读出解码纹理的描述与设备一致性，并更新诊断计数。
    fn describe_source(&mut self, source: &ID3D11Texture2D) -> (D3D11_TEXTURE2D_DESC, bool) {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` 是合法的输出参数。
        unsafe { source.GetDesc(&mut desc) };
        // 记下解码纹理的真实格式与尺寸 —— 「画面有没有真的上屏」
        // 取决于它与交换链是否对得上（§3.10）。
        self.info.source_format = desc.Format.0;
        self.info.source_width = desc.Width;
        self.info.source_height = desc.Height;
        let same_device = match unsafe { source.GetDevice() } {
            Ok(device) => device == self.device,
            Err(err) => {
                tracing::warn!(error = %err, "读不到解码纹理的设备,本帧改为清屏");
                false
            }
        };
        (desc, same_device)
    }

    /// 把当前后备缓冲呈现到屏幕。
    fn present_to_screen(&mut self) -> Result<(), RenderError> {
        let (sync_interval, flags) = if self.vrr {
            // 不等垂直刷新：帧可以落在刷新周期中间（VRR 的呈现方式）。
            (0, DXGI_PRESENT_ALLOW_TEARING)
        } else {
            // 经典 vsync：等下一个垂直刷新。
            (1, DXGI_PRESENT(0))
        };

        // SAFETY: `Present` 是 DXGI 的呈现调用，参数都是标量。
        let hr = unsafe { self.swapchain.Present(sync_interval, flags) };
        if hr.is_err() {
            let code = hr.0;
            if code == DXGI_ERROR_DEVICE_REMOVED.0 || code == DXGI_ERROR_DEVICE_RESET.0 {
                tracing::error!(code, "渲染设备丢失");
                return Err(RenderError::DeviceLost);
            }
            tracing::warn!(code, "Present 失败");
        }
        Ok(())
    }

    /// 取当前后备缓冲的渲染目标视图（首次遇到该索引时才创建）。
    ///
    /// flip-model 交换链的 `GetBuffer(i)` 只在 `i` 等于当前后台缓冲索引时可用，
    /// 因此视图必须按索引**懒创建**。
    fn back_buffer_view(&mut self, index: usize) -> Result<ID3D11RenderTargetView, RenderError> {
        if let Some(Some(rtv)) = self.rtvs.get(index) {
            return Ok(rtv.clone());
        }
        let buffer: ID3D11Texture2D =
            unsafe { self.swapchain.GetBuffer(index as u32) }.map_err(|err| {
                tracing::error!(error = %err, index, "获取后备缓冲失败");
                RenderError::SwapChainFailed {
                    reason: format!("获取后备缓冲失败: HRESULT={:#x}", err.code().0),
                }
            })?;
        let mut rtv = None;
        unsafe {
            self.device
                .CreateRenderTargetView(&buffer, None, Some(&mut rtv))
        }
        .map_err(|err| {
            tracing::error!(error = %err, index, "创建渲染目标视图失败");
            RenderError::SwapChainFailed {
                reason: "创建渲染目标视图失败".to_string(),
            }
        })?;
        let rtv = rtv.expect("CreateRenderTargetView 成功后必须返回视图");
        if let Some(slot) = self.rtvs.get_mut(index) {
            *slot = Some(rtv.clone());
        }
        Ok(rtv)
    }

    /// 读取交换链呈现统计（§3.10 测试：丢帧统计）。
    pub(crate) fn frame_statistics(&self) -> Option<FrameStatistics> {
        let mut stats = DXGI_FRAME_STATISTICS::default();
        // SAFETY: `stats` 是合法的输出结构。
        unsafe { self.swapchain.GetFrameStatistics(&mut stats) }.ok()?;
        Some(FrameStatistics {
            present_count: stats.PresentCount,
            sync_refresh_count: stats.SyncRefreshCount,
            present_refresh_count: stats.PresentRefreshCount,
            dropped: 0,
        })
    }
}

/// 创建默认的硬件 D3D11 设备。
fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext), RenderError> {
    let mut device = None;
    let mut device_context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    // SAFETY: 输出参数都是合法的可选项；特性级别传 `None` 表示用默认档位。
    let created = unsafe {
        windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut device_context),
        )
    };
    created.map_err(|err| {
        tracing::error!(error = %err, "创建 D3D11 设备失败");
        RenderError::SwapChainFailed {
            reason: "创建 D3D11 设备失败".to_string(),
        }
    })?;
    match (device, device_context) {
        (Some(device), Some(device_context)) => Ok((device, device_context)),
        _ => Err(RenderError::SwapChainFailed {
            reason: "D3D11CreateDevice 没有返回设备".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{D3D11Renderer, RenderConfig, Renderer, WindowHandle};
    use beam_proto::{DecodedFrame, PlatformHandle};

    /// 回归测试：flip-model 交换链只暴露**当前**后台缓冲索引。
    ///
    /// 实测（本机 DXGI）：`GetBuffer(i)` 只有在 `i == GetCurrentBackBufferIndex()`
    /// 时才成功，否则返回 `DXGI_ERROR_INVALID_CALL`。
    /// 这正是本层必须**懒创建**渲染目标视图的原因（见 `back_buffer_view`）。
    #[test]
    #[ignore = "需要真实显示器与交互式桌面会话"]
    fn flip_model_only_exposes_current_back_buffer_index() {
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIFactory2, DXGI_SWAP_CHAIN_DESC1,
        };

        let hwnd = create_test_window(640, 360);
        let (device, _context) = create_device().expect("应当能创建设备");
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1() }.expect("应当能建工厂");
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 640,
            Height: 360,
            Format: dxgi_format(SwapChainFormat::Bgra8),
            Stereo: FALSE,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let chain = unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) }
            .expect("应当能创建交换链");
        let chain3 = chain
            .cast::<IDXGISwapChain3>()
            .expect("应当能取到 IDXGISwapChain3");

        let current = unsafe { chain3.GetCurrentBackBufferIndex() };
        let other = 1 - current;
        let ok: Result<ID3D11Texture2D, _> = unsafe { chain3.GetBuffer(current) };
        let bad: Result<ID3D11Texture2D, _> = unsafe { chain3.GetBuffer(other) };
        let ok_result = ok.is_ok();
        let bad_code = bad.err().map(|e| e.code().0);
        println!(
            "FLIP current={current} GetBuffer(current)=ok:{ok_result} GetBuffer(other)=err:{bad_code:?}"
        );
        assert!(ok_result, "当前后台缓冲必须可取");
        assert!(
            bad_code.is_some(),
            "非当前后台缓冲索引在 flip-model 下应当被拒绝"
        );

        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
    }

    /// 创建一个用于冒烟测试的真实窗口。
    fn create_test_window(width: i32, height: i32) -> HWND {
        use windows::core::w;
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW,
            WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        /// 转发到系统默认窗口过程。
        unsafe extern "system" fn wnd_proc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        // SAFETY: 这里的字符串字面量在进程生命周期内一直有效。
        let class_name = w!("BeamRenderSmokeTest");
        // SAFETY: 窗口过程与类名都是合法的；注册失败（类已存在）可以忽略。
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("BeamRenderSmokeTest"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                0,
                0,
                width,
                height,
                None,
                None,
                None,
                None,
            )
            .expect("应当能创建测试窗口")
        }
    }

    /// 真机冒烟测试：真的建 flip-model 交换链并呈现一帧。
    ///
    /// 默认 `#[ignore]`：CI / 无桌面会话的环境里建交换链必然失败。
    /// 本机验证方式：
    ///
    /// ```text
    /// cargo test -p beam-render --all-features flip_model_swap_chain_presents -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "需要真实显示器与交互式桌面会话"]
    fn flip_model_swap_chain_presents_a_frame() {
        let hwnd = create_test_window(640, 360);

        let mut renderer = D3D11Renderer::new();
        renderer
            .attach(
                WindowHandle(hwnd.0 as usize),
                RenderConfig {
                    width: 640,
                    height: 360,
                    refresh_rate: 60,
                    vrr: true,
                    flip_model: true,
                    format: SwapChainFormat::Bgra8,
                },
            )
            .expect("flip-model 交换链应当能创建");
        assert!(renderer.attached());

        // 没有解码纹理时也要走完整条呈现路径（清屏 + Present）。
        let frame = DecodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            width: 640,
            height: 360,
            is_key: true,
            surface: PlatformHandle::NONE,
        };
        renderer.present(&frame).expect("应当能上屏");
        assert_eq!(renderer.presented(), 1);

        // 解码线程 → 单槽 → 渲染线程：单槽交换也走一遍真实呈现。
        let handle = renderer.frame_handle();
        handle.submit(frame);
        assert!(renderer.present_pending().expect("单槽上屏应当成功"));
        assert_eq!(renderer.presented(), 2);

        // 丢帧统计（§3.10 测试项）：DXGI 可能返回 DXGI_ERROR_FRAME_STATISTICS_DISJOINT
        // （呈现的帧还不够多），这时没有统计可读——如实反映为 None。
        match renderer.frame_statistics() {
            Some(stats) => println!("PROBE frame_statistics={stats:?}"),
            None => {
                println!("PROBE frame_statistics=暂不可用(DXGI_ERROR_FRAME_STATISTICS_DISJOINT)")
            }
        }

        // SAFETY: 销毁自己创建的窗口。
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
    }
}
