//! WGC（Windows Graphics Capture）后端（§3.5 主路径）。
//!
//! ## 线程模型
//!
//! `windows-capture` 1.5 在**自己的线程**上跑 WGC 的消息循环；
//! 帧到达回调（`on_frame_arrived`）只做两件事：
//!
//! 1. 把 WGC 帧纹理 **GPU→GPU 拷贝**到本层自己拥有的纹理上；
//! 2. 把纹理 + 时间戳发布到「最新帧」单槽。
//!
//! 回调里不做颜色转换、不做编码、不拷到 CPU（§3.5 边界），
//! 也不做任何可能阻塞的操作——编码节拍线程通过
//! [`WgcCaptureSource::latest`](crate::CaptureSource::latest) 非阻塞取最新帧。
//!
//! ## 为什么要做一次 GPU 拷贝
//!
//! WGC 的帧池是**池化**的：回调返回后，帧对象被释放，池会把同一块纹理
//! 交给下一帧复用。若直接把池化纹理交给编码线程，编码线程读到的可能是
//! 已经被下一帧覆盖的内容。因此这里在回调内把内容拷到**一块每帧新建的**
//! 目标纹理上，交给编码线程的是一块**生命周期完全由消费者掌控**的纹理：
//! 消费者释放它之前，内容不会被改写（也就不会读到撕裂帧）。
//!
//! 代价是每帧一次 D3D11 纹理创建（驱动侧通常走内存池，开销在几十微秒量级）。
//! **该开销没有在本机实测过**——§3.5 的性能预算需要真机多屏 / 高刷环境才能测。
//!
//! ## 版本桥接（windows-capture 1.5 ↔ windows 0.62）
//!
//! `windows-capture` 1.5 依赖 `windows 0.61`，而本 crate 对外暴露的
//! [`Texture`](crate::Texture) 是 `windows 0.62` 的 `ID3D11Texture2D`
//! ——必须与 beam-encode 同型，否则 §6 的「capture→encode 同一设备零拷贝」无从谈起。
//!
//! D3D11 是操作系统定义的稳定 COM ABI：两个版本的 `ID3D11Texture2D`
//! IID 相同、vtable 布局相同，因此同一个 COM 指针可以安全地被另一版本的
//! 绑定包装。桥接只做「AddRef 一次 + 换一层包装」，不碰 vtable 内容。
//!
//! ## `MinUpdateInterval` 与 SDK 版本（重要，别被 SDK 号误导）
//!
//! §3.5 写的是「需要 SDK 26100（Windows 11 24H2）」。这句话对 **C/C++ 编译**
//! 成立，但对 Rust 不成立：
//!
//! - `windows` 0.62 的绑定是**预生成**的，已经包含
//!   `GraphicsCaptureSession::SetMinUpdateInterval`（编译期不依赖本机 SDK）；
//! - 真正决定这个能力有没有的是**运行时 Windows 版本**：
//!   Windows 10 22H2 上没有 `MinUpdateInterval` 属性，Windows 11 24H2 及以上有。
//!
//! 因此本层的做法是**运行时探测 + 明确降级**：用
//! `ApiInformation::IsPropertyPresent` 判定属性是否存在，
//! 不存在就用系统默认更新间隔，并通过
//! [`CaptureDegradation::MinUpdateIntervalUnsupported`](crate::CaptureDegradation::MinUpdateIntervalUnsupported)
//! 如实记录——**不假装设置成功**。
//!
//! ## 性能预算
//!
//! §3.5 的预算是「抓屏+取纹理 T1 ≤1.5 ms、T2/T3 ≤1 ms」。
//! 本实现按预算设计（单槽交换、回调内不做重活、只做一次 GPU 拷贝），
//! **但没有在本机实测过该预算**——需要真实多显示器 / 高刷新率环境才能测。

// 本模块需要 unsafe：D3D11 是裸 COM API——创建纹理、GPU 拷贝、读取资源描述
// 都是 unsafe 调用；此外还要把 windows 0.61 的 COM 指针桥接成 windows 0.62
// 的包装类型（COM 引用计数由包装类型负责）。
#![allow(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Foundation::Metadata::ApiInformation;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
};

use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{self, InternalCaptureControl};
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use windows_legacy::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};

use crate::error::CapError;
use crate::source::{
    now_us, select_monitor, CaptureConfig, CaptureDegradation, CursorMode, FrameChannel,
    PublishedFrame, Texture,
};

/// windows-capture 1.5 使用的错误类型（`windows` 0.61 的 `windows_result`）。
type LegacyError = windows_legacy::core::Error;

/// WGC 后端句柄。
pub(crate) struct WgcBackend {
    /// 捕获线程的控制句柄（`stop` 时终止线程并 join）。
    control: Option<CaptureControl<WgcHandler, LegacyError>>,
    /// 「最新帧」单槽。
    channel: Arc<FrameChannel>,
}

impl WgcBackend {
    /// 按配置启动 WGC 抓屏。
    ///
    /// 失败时返回 §3.5 定义的码位（`E5001` / `E5002` / `E5003`）。
    pub(crate) fn start(
        cfg: &CaptureConfig,
        cursor: CursorMode,
        degradations: &mut Vec<CaptureDegradation>,
    ) -> Result<Self, CapError> {
        let monitor = select_monitor(cfg)?;
        let cursor_settings = cursor_settings(cursor, degradations);
        let interval_settings = interval_settings(cfg, degradations);

        let channel = Arc::new(FrameChannel::new());
        let settings = Settings::new(
            monitor,
            cursor_settings,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            interval_settings,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            channel.clone(),
        );

        let control = WgcHandler::start_free_threaded(settings).map_err(map_capture_start_error)?;
        tracing::info!(
            display_id = cfg.display_id,
            min_update_interval = cfg.min_update_interval,
            "WGC 抓屏已启动"
        );
        Ok(Self {
            control: Some(control),
            channel,
        })
    }

    /// 取最新一帧（非阻塞）。
    pub(crate) fn latest(&mut self) -> Option<crate::source::CapturedFrame> {
        self.channel
            .take()
            .map(|frame| crate::source::CapturedFrame {
                tex: frame.tex,
                ts_us: frame.ts_us,
                // WGC 只在画面变化时投递新帧（显示器捕获是内容驱动的），
                // 因此投递出来的每一帧相对上一帧都是「脏」的。
                dirty: true,
            })
    }

    /// 停止抓屏（终止捕获线程）。
    pub(crate) fn stop(&mut self) {
        if let Some(control) = self.control.take() {
            if let Err(err) = control.stop() {
                tracing::warn!(error = %err, "WGC 捕获线程停止失败");
            }
        }
    }

    /// 捕获线程是否已经结束（异常退出时用于诊断）。
    pub(crate) fn is_finished(&self) -> bool {
        self.control
            .as_ref()
            .map(CaptureControl::is_finished)
            .unwrap_or(true)
    }
}

/// WGC 帧到达回调的处理体。
struct WgcHandler {
    /// WGC 自己创建的 D3D11 设备（目标纹理必须在同一设备上创建）。
    device: ID3D11Device,
    /// 设备上下文（GPU 拷贝在它上面排队）。
    device_context: ID3D11DeviceContext,
    /// 「最新帧」单槽。
    channel: Arc<FrameChannel>,
}

impl GraphicsCaptureApiHandler for WgcHandler {
    /// 回调参数：把单槽带进捕获线程。
    type Flags = Arc<FrameChannel>;
    /// 回调错误类型（与 windows-capture 1.5 保持同一套 `windows_result`）。
    type Error = LegacyError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            device: ctx.device,
            device_context: ctx.device_context,
            channel: ctx.flags,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let ts_us = now_us();
        // SAFETY: `as_raw_texture` 返回的引用在 `frame` 存活期间有效；
        // 这里只借用它做一次 GPU 拷贝，不把引用带出回调。
        let source = unsafe { frame.as_raw_texture() };
        // 每帧新建一块目标纹理：这样交给消费者的是一块**独占**的纹理，
        // 消费者释放它之前不会被下一帧覆盖（复用同一块纹理会导致读到撕裂帧）。
        let destination = self.create_destination(source)?;
        // SAFETY: 目标纹理与源纹理由同一设备创建，尺寸与格式一致，
        // 满足 D3D11 `CopyResource` 的前置条件。
        unsafe { self.device_context.CopyResource(&destination, source) };

        // AddRef 一份交给消费者；回调结束时本地这份引用被释放，
        // 消费者持有的那份保证纹理内容在它读完之前不被回收。
        let texture = bridge_texture(&destination);
        self.channel.publish(PublishedFrame {
            tex: texture,
            ts_us,
        });
        Ok(())
    }
}

impl WgcHandler {
    /// 新建一块与源纹理同尺寸同格式的目标纹理。
    fn create_destination(&self, source: &ID3D11Texture2D) -> Result<ID3D11Texture2D, LegacyError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` 是合法的输出参数。
        unsafe { source.GetDesc(&mut desc) };

        let target_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: desc.SampleDesc,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut destination = None;
        // SAFETY: 描述符有效；输出参数在调用返回时被写入。
        unsafe {
            self.device
                .CreateTexture2D(&target_desc, None, Some(&mut destination))?
        };
        Ok(destination.expect("CreateTexture2D 成功后必须返回纹理"))
    }
}

/// 把 windows-capture 1.5（`windows 0.61`）交出的纹理转成本 crate 的
/// `windows 0.62` 纹理类型。
///
/// 详见模块头部「版本桥接」一节：D3D11 的 COM ABI 由操作系统定义，
/// 两个绑定版本共享同一 IID 与 vtable 布局。
fn bridge_texture(legacy: &ID3D11Texture2D) -> Texture {
    // AddRef 一次，把这一份引用所有权交给 0.62 的包装类型（析构时由它 Release）。
    let owned: ID3D11Texture2D = Clone::clone(legacy);
    let raw = windows_legacy::core::Interface::into_raw(owned);
    // SAFETY: `raw` 是合法的 `ID3D11Texture2D` COM 指针，且本函数持有一份引用。
    unsafe { windows::core::Interface::from_raw(raw) }
}

/// 光标模式 → WGC 光标设置。
fn cursor_settings(
    cursor: CursorMode,
    degradations: &mut Vec<CaptureDegradation>,
) -> CursorCaptureSettings {
    match cursor {
        CursorMode::Composited => CursorCaptureSettings::Default,
        CursorMode::Separate => {
            if cursor_capture_supported() {
                // 关闭光标合成 → 光标可以走独立通道。
                CursorCaptureSettings::WithoutCursor
            } else {
                degradations.push(CaptureDegradation::CursorCaptureUnsupported);
                tracing::warn!("当前系统不支持单独开关光标,已回退为光标合成进画面");
                CursorCaptureSettings::Default
            }
        }
    }
}

/// `MinUpdateInterval` 能力探测。
///
/// windows 0.62 生成的绑定里**没有** `GraphicsCaptureItem::IsCursorCaptureSupported`
/// 这样的静态方法（只有 `GraphicsCaptureSession::SetIsCursorCaptureEnabled` 实例方法），
/// 因此按 WinRT 的标准做法用 `ApiInformation::IsPropertyPresent` 探测属性是否存在，
/// 判定口径与 windows-capture 1.5 内部完全一致。
fn cursor_capture_supported() -> bool {
    ApiInformation::IsPropertyPresent(
        &HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureSession"),
        &HSTRING::from("IsCursorCaptureEnabled"),
    )
    .unwrap_or(false)
}

/// `MinUpdateInterval` 能力探测（§3.5 平台限制）。
fn min_update_interval_supported() -> bool {
    ApiInformation::IsPropertyPresent(
        &HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureSession"),
        &HSTRING::from("MinUpdateInterval"),
    )
    .unwrap_or(false)
}

/// `min_update_interval` → WGC 最小更新间隔设置。
///
/// 不支持时**明确降级**为系统默认间隔，并记录
/// [`CaptureDegradation::MinUpdateIntervalUnsupported`]——不假装设置成功。
fn interval_settings(
    cfg: &CaptureConfig,
    degradations: &mut Vec<CaptureDegradation>,
) -> MinimumUpdateIntervalSettings {
    let Some(requested_ms) = cfg.min_update_interval else {
        return MinimumUpdateIntervalSettings::Default;
    };
    if min_update_interval_supported() {
        MinimumUpdateIntervalSettings::Custom(Duration::from_millis(u64::from(requested_ms)))
    } else {
        degradations.push(CaptureDegradation::MinUpdateIntervalUnsupported { requested_ms });
        tracing::warn!(
            requested_ms,
            "当前系统不支持 MinUpdateInterval(需 Windows 11 24H2),已降级为系统默认更新间隔"
        );
        MinimumUpdateIntervalSettings::Default
    }
}

/// 把 WGC 会话初始化失败映射到 §3.5 的码位。
fn map_start_error(err: graphics_capture_api::Error) -> CapError {
    match err {
        graphics_capture_api::Error::WindowsError(inner) => map_hresult(inner),
        graphics_capture_api::Error::DirectXError(inner) => {
            tracing::error!(error = %inner, "WGC 创建 D3D11 设备失败");
            CapError::DeviceLost
        }
        graphics_capture_api::Error::WindowError(inner) => {
            tracing::error!(error = %inner, "WGC 捕获项创建失败");
            CapError::NoDisplay
        }
        graphics_capture_api::Error::Unsupported => {
            tracing::error!("当前系统不支持 Windows Graphics Capture");
            CapError::NoDisplay
        }
        graphics_capture_api::Error::MinimumUpdateIntervalUnsupported => {
            // 理论上到不了这里：start 之前已经用同一套 ApiInformation 判定过能力。
            tracing::error!("WGC 拒绝了 MinUpdateInterval 设置");
            CapError::NoDisplay
        }
        other => {
            tracing::error!(error = %other, "WGC 会话初始化失败");
            CapError::NoDisplay
        }
    }
}

/// 把 WGC 启动过程的失败映射到 §3.5 的码位。
fn map_capture_start_error(
    err: windows_capture::capture::GraphicsCaptureApiError<LegacyError>,
) -> CapError {
    match err {
        windows_capture::capture::GraphicsCaptureApiError::GraphicsCaptureApiError(inner) => {
            map_start_error(inner)
        }
        windows_capture::capture::GraphicsCaptureApiError::DirectXError(inner) => {
            tracing::error!(error = %inner, "WGC 创建 D3D11 设备失败");
            CapError::DeviceLost
        }
        windows_capture::capture::GraphicsCaptureApiError::ItemConvertFailed => {
            tracing::error!("抓屏目标转换失败");
            CapError::NoDisplay
        }
        windows_capture::capture::GraphicsCaptureApiError::NewHandlerError(inner) => {
            map_hresult(inner)
        }
        windows_capture::capture::GraphicsCaptureApiError::FrameHandlerError(inner) => {
            map_hresult(inner)
        }
        other => {
            tracing::error!(error = %other, "WGC 抓屏启动失败");
            CapError::NoDisplay
        }
    }
}

/// 把原生 HRESULT 映射到 §3.5 的三个码位。
///
/// §3.5 只定义了 `E5001` / `E5002` / `E5003`，因此这里没有第四个「通用失败」码：
///
/// - `E_ACCESSDENIED`（0x80070005）：WGC 上最常见的原因是目标应用处于**独占全屏**
///   模式，此时应切 DXGI 后端 → `E5002`；
/// - `DXGI_ERROR_DEVICE_REMOVED` / `DEVICE_RESET` / `ACCESS_LOST` → `E5003`；
/// - 其余（拿不到捕获项、目标不存在、WGC 不支持）→ `E5001`。
///
/// 无法归类的原始错误码会记进日志，不会被吞掉。
///
/// 这里比较的是 HRESULT 的原始数值：windows-capture 1.5 用的是 `windows` 0.61
/// 的 `windows_result`，与本 crate 的 `windows` 0.62 是两套类型，但 HRESULT 数值一致。
fn map_hresult(err: LegacyError) -> CapError {
    let code = err.code().0;
    if code == E_ACCESSDENIED.0 {
        return CapError::ExclusiveFullscreen;
    }
    if code == DXGI_ERROR_DEVICE_REMOVED.0
        || code == DXGI_ERROR_DEVICE_RESET.0
        || code == DXGI_ERROR_ACCESS_LOST.0
    {
        return CapError::DeviceLost;
    }
    tracing::error!(code, "抓屏后端原生调用失败");
    CapError::NoDisplay
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个带指定 HRESULT 的原生错误。
    fn legacy_error(code: i32) -> LegacyError {
        LegacyError::new(windows_legacy::core::HRESULT(code), "test")
    }

    #[test]
    fn hresult_mapping_follows_doc_codes() {
        // E_ACCESSDENIED → E5002（独占全屏，应切 DXGI）。
        let denied = legacy_error(E_ACCESSDENIED.0);
        assert_eq!(map_hresult(denied).code(), 5002);

        // 设备丢失类 → E5003。
        let removed = legacy_error(DXGI_ERROR_DEVICE_REMOVED.0);
        assert_eq!(map_hresult(removed).code(), 5003);
        let reset = legacy_error(DXGI_ERROR_DEVICE_RESET.0);
        assert_eq!(map_hresult(reset).code(), 5003);
        let lost = legacy_error(DXGI_ERROR_ACCESS_LOST.0);
        assert_eq!(map_hresult(lost).code(), 5003);

        // 其余 → E5001。
        let other = legacy_error(0x80004005u32 as i32);
        assert_eq!(map_hresult(other).code(), 5001);
    }

    #[test]
    fn cursor_capability_probe_does_not_panic() {
        // 能力探测只读属性表，任何结果都不应 panic。
        let _ = cursor_capture_supported();
    }

    #[test]
    fn min_update_interval_probe_does_not_panic() {
        let _ = min_update_interval_supported();
    }

    /// 真机冒烟测试：真的用 WGC 抓一帧。
    ///
    /// 默认 `#[ignore]`：CI / 无桌面会话的环境里抓屏必然失败。
    /// 本机验证方式：
    ///
    /// ```text
    /// cargo test -p beam-capture --all-features wgc_real_capture -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "需要真实显示器与交互式桌面会话"]
    fn wgc_real_capture_produces_a_frame() {
        use crate::source::{CaptureBackend, CaptureConfig, CaptureSource, WgcCaptureSource};

        let mut source = WgcCaptureSource::new();
        let cfg = CaptureConfig {
            backend: CaptureBackend::Wgc,
            display_id: None,
            min_update_interval: None,
            target_fps: 60,
        };
        source.start(cfg).expect("WGC 抓屏应当能启动");
        assert_eq!(source.backend_kind(), Some("wgc"));

        let mut captured = None;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if let Some(frame) = source.latest() {
                captured = Some(frame);
                break;
            }
        }
        source.stop();

        let frame = captured.expect("应当在 3 秒内抓到至少一帧");
        let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` 是合法的输出参数。
        unsafe { frame.tex.GetDesc(&mut desc) };
        assert!(desc.Width > 0, "纹理宽度应当有效");
        assert!(desc.Height > 0, "纹理高度应当有效");
        assert!(frame.ts_us > 0, "时间戳应当有效");
        tracing::info!(
            width = desc.Width,
            height = desc.Height,
            ts_us = frame.ts_us,
            "WGC 真机抓屏成功"
        );
    }

    /// 真机冒烟测试：`min_update_interval` 的能力探测与降级记录必须一致。
    ///
    /// §3.5 平台限制要求：不支持时**明确降级并如实反映**，不假装设置成功。
    /// 本机运行时（Windows 11 build 26200）支持 `MinUpdateInterval`，
    /// 因此这里应当既没有降级记录、抓屏也能正常启动。
    #[test]
    #[ignore = "需要真实显示器与交互式桌面会话"]
    fn wgc_min_update_interval_degradation_is_honest() {
        use crate::source::{
            CaptureBackend, CaptureConfig, CaptureDegradation, CaptureSource, WgcCaptureSource,
        };

        let supported = min_update_interval_supported();
        let mut source = WgcCaptureSource::new();
        let cfg = CaptureConfig {
            backend: CaptureBackend::Wgc,
            display_id: None,
            min_update_interval: Some(8),
            target_fps: 60,
        };
        source.start(cfg).expect("WGC 抓屏应当能启动");
        let degraded = source
            .degradations()
            .iter()
            .any(|d| matches!(d, CaptureDegradation::MinUpdateIntervalUnsupported { .. }));
        source.stop();
        assert_eq!(
            degraded, !supported,
            "能力探测({supported})与降级记录({degraded})必须一致,不允许假装设置成功"
        );
    }
}
