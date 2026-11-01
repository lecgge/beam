//! 渲染器契约与 D3D11 flip-model 实现（§3.10）。
//!
//! - 解码纹理直接交给 D3D11 flip-model 交换链，支持可变刷新率；
//! - 叠加层由 Tauri 窗口负责，本 crate 不做 UI。
//!
//! ## 线程模型（§3.10）
//!
//! ```text
//! 解码线程  ──submit()──▶  单槽（无锁，只留最新一帧）  ──take()──▶  渲染线程（按 vblank 上屏）
//! ```
//!
//! - [`FrameHandle::submit`] 由解码线程调用，把解码帧覆盖式写进单槽；
//! - 渲染线程（[`RenderThread`]）按节拍调用 [`D3D11Renderer::present_pending`]，
//!   取走单槽里的最新帧并真正上屏（`Present` 本身与垂直刷新对齐）。
//!
//! ## 上屏等待
//!
//! §3.10 要求「上屏等待平均半帧」：帧在刷新周期内均匀到达，
//! 等下一个 vblank 的平均等待就是半帧。这是设计性质，**没有在本机实测**。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use beam_proto::DecodedFrame;

use crate::error::RenderError;
use crate::slot::FrameSlot;

#[cfg(feature = "d3d11")]
use crate::d3d11;

/// 一次呈现的诊断信息（§3.10：区分「画面真的上屏」与「降级清屏」）。
///
/// ## 为什么需要它
///
/// [`Renderer::present`] 成功返回**不代表画面上屏了**：
/// 解码纹理的格式 / 尺寸 / 设备与交换链对不上时，渲染层会退化成
/// 「清屏 + 照样走完整条呈现路径」（见 `d3d11.rs` 的 `present`）——
/// 不报错，但用户看到的是空白。
///
/// 只有一个码位、两种含义 ⇒ 无法定位。因此这里把「真的把解码纹理
/// 搬进了交换链」与「降级清屏」分开计数，并带上两边的真实格式，
/// 让上层（以及真机验证入口）能**证实**画面确实上屏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PresentInfo {
    /// 总呈现次数。
    pub presents: u64,
    /// 真的把解码纹理搬进交换链的次数。
    pub textured_presents: u64,
    /// 因格式 / 尺寸 / 设备不匹配而降级成清屏的次数。
    pub cleared_presents: u64,
    /// 解码纹理的 DXGI 格式（DXGI 四字符码的整数值；`0` = 没有解码纹理）。
    pub source_format: i32,
    /// 交换链后备缓冲的 DXGI 格式。
    pub swapchain_format: i32,
    /// 解码纹理的宽（`0` = 没有解码纹理）。
    pub source_width: u32,
    /// 解码纹理的高。
    pub source_height: u32,
}

impl PresentInfo {
    /// 交换链格式与解码纹理格式是否一致。
    ///
    /// 不一致 ⇒ 每一帧都会走「降级清屏」分支（画面上不了屏）。
    pub fn formats_match(&self) -> bool {
        self.source_format != 0 && self.source_format == self.swapchain_format
    }
}

/// 渲染器（§3.10 `Renderer::present(&DecodedFrame)`）。
pub trait Renderer {
    /// 上屏一帧。
    fn present(&mut self, f: &DecodedFrame) -> Result<(), RenderError>;

    /// 累计「画面没能上屏」的帧数（§3.10 丢帧统计）。
    ///
    /// 两种来源：
    /// - 单槽覆盖：解码快于上屏，旧帧被覆盖（`submit` / `present_pending` 那条路）；
    /// - 降级清屏：解码纹理与交换链对不上，本帧只清了屏（见 `PresentInfo`）。
    ///
    /// 默认返回 `0`：没有丢帧概念的渲染器。
    fn dropped_frames(&self) -> u64 {
        0
    }
}

/// 交换链后备缓冲的格式（§3.10）。
///
/// ## 为什么可选
///
/// D3D11VA **视频解码**路径的原生输出是 `DXGI_FORMAT_420Opaque`
/// （D3D11 视频解码器只支持这两种格式，改不了）。而解码纹理要
/// **零拷贝**进交换链，交换链的格式就必须与它一致。
///
/// BGRA 是「常规桌面窗口」的格式，好处是任何窗口都能用；代价是
/// 解码纹理（4:2:0 双平面）与它格式不同 ⇒ 只能转换，不能直接拷贝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwapChainFormat {
    /// `DXGI_FORMAT_B8G8R8A8_UNORM`：普通桌面窗口的格式。
    #[default]
    Bgra8,
    /// `DXGI_FORMAT_NV12`：4:2:0 双平面视频格式。
    Nv12,
    /// `DXGI_FORMAT_420Opaque`：4:2:0 不透明视频格式（视频解码器的原生格式）。
    Opaque420,
}

impl SwapChainFormat {
    /// 对应的 DXGI 格式码（`DXGI_FORMAT` 的整数值）。
    ///
    /// 数值取自 DXGI 的格式枚举；`DXGI_FORMAT_420Opaque` 的 windows
    /// 绑定里不一定有这个常量，所以这里直接写常量值（103）。
    pub fn dxgi_code(self) -> i32 {
        match self {
            SwapChainFormat::Bgra8 => 87,      // DXGI_FORMAT_B8G8R8A8_UNORM
            SwapChainFormat::Nv12 => 70,       // DXGI_FORMAT_NV12
            SwapChainFormat::Opaque420 => 103, // DXGI_FORMAT_420Opaque
        }
    }

    /// 可读的格式名（用于日志与验证输出）。
    pub fn name(self) -> &'static str {
        match self {
            SwapChainFormat::Bgra8 => "DXGI_FORMAT_B8G8R8A8_UNORM",
            SwapChainFormat::Nv12 => "DXGI_FORMAT_NV12",
            SwapChainFormat::Opaque420 => "DXGI_FORMAT_420Opaque",
        }
    }
}

/// 渲染配置（§3.10：可变刷新率）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    /// 窗口宽。
    pub width: u32,
    /// 窗口高。
    pub height: u32,
    /// 目标刷新率（用于帧节奏计算与诊断对比）。
    ///
    /// DXGI 不允许直接指定交换链的刷新率（由显示器与驱动决定），
    /// 因此这一项只用于帧节奏与「实测刷新率是否达到目标」的对比。
    pub refresh_rate: u32,
    /// 是否启用可变刷新率（VRR）。
    pub vrr: bool,
    /// 是否启用 flip-model。
    pub flip_model: bool,
    /// 交换链后备缓冲的格式（§3.10）。
    ///
    /// 默认 BGRA（普通桌面窗口）。要零拷贝承接 D3D11VA 的
    /// `DXGI_FORMAT_420Opaque` 解码纹理，就要改成
    /// [`SwapChainFormat::Opaque420`]（并让交换链高度与解码纹理对齐）。
    pub format: SwapChainFormat,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
            vrr: true,
            flip_model: true,
            format: SwapChainFormat::default(),
        }
    }
}

/// 宿主窗口句柄。
///
/// Windows 上对应 `HWND`（整数值形式）；用整型封装的目的是让本 crate 的
/// 公开 API 保持平台中立（非 Windows 平台也能编译通过）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowHandle(pub usize);

/// 交换链呈现统计（§3.10 测试：丢帧统计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameStatistics {
    /// 交换链累计呈现次数。
    pub present_count: u32,
    /// 与垂直刷新同步的呈现次数。
    pub sync_refresh_count: u32,
    /// 实际按显示器刷新节奏呈现的次数（VRR 生效时与上一项不同）。
    pub present_refresh_count: u32,
    /// 本层统计的丢帧数。
    pub dropped: u64,
}

/// 解码线程 → 渲染线程的提交句柄（§3.10 单槽）。
///
/// 克隆廉价，可以在解码线程与渲染线程之间自由传递。
#[derive(Clone)]
pub struct FrameHandle {
    /// 共享的单槽。
    slot: Arc<FrameSlot<DecodedFrame>>,
}

impl FrameHandle {
    /// 提交一帧（覆盖旧帧）。
    ///
    /// 解码线程调用：慢消费者只会拿到最新的一帧，不会积压延迟。
    pub fn submit(&self, frame: DecodedFrame) {
        self.slot.publish(frame);
    }
}

/// 默认 D3D11 渲染器。
///
/// 未开启 `d3d11` feature 时不触碰原生设备，`present` 返回 `E10014`
/// （**非文档码位**，只表示「后端未编译进来」）。
#[derive(Default)]
pub struct D3D11Renderer {
    /// 当前配置。
    cfg: Option<RenderConfig>,
    /// D3D11 交换链（`attach` 之后才有）。
    #[cfg(feature = "d3d11")]
    backend: Option<d3d11::SwapChain>,
    /// 解码线程 → 渲染线程的单槽。
    slot: Arc<FrameSlot<DecodedFrame>>,
    /// 已上屏帧数。
    presented: u64,
    /// 已丢弃帧数。
    dropped: u64,
}

impl std::fmt::Debug for D3D11Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("D3D11Renderer")
            .field("config", &self.cfg)
            .field("attached", &self.attached())
            .field("presented", &self.presented)
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl D3D11Renderer {
    /// 新建渲染器（尚未绑定窗口）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&RenderConfig> {
        self.cfg.as_ref()
    }

    /// 是否已经绑定窗口（交换链已创建）。
    pub fn attached(&self) -> bool {
        #[cfg(feature = "d3d11")]
        {
            self.backend.is_some()
        }
        #[cfg(not(feature = "d3d11"))]
        {
            false
        }
    }

    /// 设置配置。
    ///
    /// 需要在 [`D3D11Renderer::attach`] 之前设置；交换链建好之后再改不生效。
    pub fn configure(&mut self, cfg: RenderConfig) {
        self.cfg = Some(cfg);
    }

    /// 绑定宿主窗口，创建 D3D11 flip-model 交换链。
    ///
    /// 交换链建好之后，解码侧应当复用 [`D3D11Renderer::device`] 返回的
    /// **同一个 D3D11 设备**创建解码纹理——D3D11 不允许跨设备拷贝。
    #[cfg(feature = "d3d11")]
    pub fn attach(&mut self, window: WindowHandle, cfg: RenderConfig) -> Result<(), RenderError> {
        let hwnd = windows::Win32::Foundation::HWND(window.0 as *mut core::ffi::c_void);
        let backend = d3d11::SwapChain::create(hwnd, &cfg)?;
        self.backend = Some(backend);
        self.cfg = Some(cfg);
        Ok(())
    }

    /// 绑定宿主窗口，创建 D3D11 flip-model 交换链。
    ///
    /// 非 Windows 平台没有 D3D11，按 `E10012`（交换链创建失败）回报。
    #[cfg(not(feature = "d3d11"))]
    pub fn attach(&mut self, _window: WindowHandle, cfg: RenderConfig) -> Result<(), RenderError> {
        self.cfg = Some(cfg);
        Err(RenderError::BackendDisabled)
    }

    /// 解码侧使用的 D3D11 设备。
    ///
    /// 解码纹理必须创建在**同一个设备**上，交换链才能零拷贝地把它拷进后备缓冲。
    #[cfg(feature = "d3d11")]
    pub fn device(&self) -> Option<&windows::Win32::Graphics::Direct3D11::ID3D11Device> {
        self.backend.as_ref().map(d3d11::SwapChain::device)
    }

    /// 取走单槽里的提交句柄（供解码线程使用）。
    pub fn frame_handle(&self) -> FrameHandle {
        FrameHandle {
            slot: Arc::clone(&self.slot),
        }
    }

    /// 提交一帧到单槽（解码线程调用）。
    pub fn submit(&self, frame: DecodedFrame) {
        self.slot.publish(frame);
    }

    /// 渲染线程的一拍：取走单槽里的最新帧并上屏。
    ///
    /// 返回 `true` 表示这一拍确实上屏了；`false` 表示单槽是空的
    /// （解码线程还没产出新帧）。
    pub fn present_pending(&mut self) -> Result<bool, RenderError> {
        let Some(frame) = self.slot.take() else {
            return Ok(false);
        };
        self.present(&frame)?;
        Ok(true)
    }

    /// 已上屏帧数。
    pub fn presented(&self) -> u64 {
        self.presented
    }

    /// 已丢弃帧数。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 累计「画面没能上屏」的帧数（§3.10 丢帧统计）。
    ///
    /// = 单槽覆盖的帧数 + 降级清屏的帧数（后者见 [`PresentInfo::cleared_presents`]）。
    pub fn dropped_frames(&self) -> u64 {
        let slot_dropped = self.slot.dropped();
        #[cfg(feature = "d3d11")]
        let cleared = self
            .backend
            .as_ref()
            .map(|b| b.present_info().cleared_presents)
            .unwrap_or(0);
        #[cfg(not(feature = "d3d11"))]
        let cleared = 0;
        slot_dropped + cleared
    }

    /// 读取交换链呈现统计（丢帧统计）。
    /// 读取交换链呈现统计（丢帧统计）。
    ///
    /// ## 已知限制
    ///
    /// DXGI 的 `GetFrameStatistics` **在 flip-model 交换链上不受支持**
    /// （微软明确说明该方法不支持 flip model）⇒ 本方法在默认配置下
    /// 恒返回 `None`。
    ///
    /// 要判断「画面有没有真的上屏」，请用 [`D3D11Renderer::present_info`]：
    /// 它统计的是**真的把解码纹理搬进了交换链**的次数，不依赖 DXGI 的呈现统计。
    pub fn frame_statistics(&self) -> Option<FrameStatistics> {
        #[cfg(feature = "d3d11")]
        {
            let mut stats = self.backend.as_ref()?.frame_statistics()?;
            stats.dropped = self.dropped;
            Some(stats)
        }
        #[cfg(not(feature = "d3d11"))]
        {
            None
        }
    }

    /// 呈现诊断信息（§3.10）：真的搬纹理 vs 降级清屏 + 两边的真实格式。
    ///
    /// 没有 `d3d11` feature 时恒为默认值（没有交换链，也就没有呈现）。
    pub fn present_info(&self) -> PresentInfo {
        #[cfg(feature = "d3d11")]
        {
            self.backend
                .as_ref()
                .map(|b| b.present_info())
                .unwrap_or_default()
        }
        #[cfg(not(feature = "d3d11"))]
        {
            PresentInfo::default()
        }
    }
}

impl Renderer for D3D11Renderer {
    fn present(&mut self, f: &DecodedFrame) -> Result<(), RenderError> {
        #[cfg(feature = "d3d11")]
        {
            let Some(backend) = self.backend.as_mut() else {
                return Err(RenderError::SwapChainFailed {
                    reason: "尚未绑定窗口,没有交换链".to_string(),
                });
            };

            // §3.13「渲染」阶段：解码纹理拷进后备缓冲 + 交换链 Present。
            let _render_span = perf_span!("render");

            // `DecodedFrame::surface` 按 §4 分层说明承载平台指针：
            // 解码纹理的指针由渲染层解释成 D3D11 纹理，并且只借用、不接管所有权。
            backend.present(f.surface.raw)?;
            self.presented += 1;
            Ok(())
        }
        #[cfg(not(feature = "d3d11"))]
        {
            let _ = f;
            // E10014 不是 §3.10 定义的码位：它只表示「后端未编译进来」。
            Err(RenderError::BackendDisabled)
        }
    }

    fn dropped_frames(&self) -> u64 {
        D3D11Renderer::dropped_frames(self)
    }
}

/// 渲染线程（§3.10：解码线程 → 单槽 → 渲染线程（按 vblank））。
///
/// 线程体按 `interval_us` 的节拍从单槽取最新帧并上屏；
/// `Present` 本身与垂直刷新对齐，因此不需要额外的 vsync 等待。
pub struct RenderThread {
    /// 停止标志。
    stop: Arc<AtomicBool>,
    /// 线程句柄。
    handle: Option<JoinHandle<()>>,
}

impl RenderThread {
    /// 启动渲染线程。
    ///
    /// 返回渲染线程句柄与解码线程用的提交句柄。
    pub fn spawn(mut renderer: D3D11Renderer, interval_us: u64) -> (Self, FrameHandle) {
        let frame_handle = renderer.frame_handle();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("beam-render".to_string())
            .spawn(move || {
                // 上一次上屏的时刻（微秒），用于计算这一拍该不该上屏。
                let mut last_present_us: Option<u64> = None;
                while !flag.load(Ordering::Relaxed) {
                    let now = now_us();
                    let due = match last_present_us {
                        Some(last) => now.saturating_sub(last) >= interval_us,
                        None => true,
                    };
                    if due {
                        match renderer.present_pending() {
                            Ok(true) => last_present_us = Some(now),
                            Ok(false) => {}
                            Err(err) => {
                                tracing::warn!(error = %err, "上屏失败");
                            }
                        }
                    }
                    // 让出时间片：轮询间隔取节拍与 1 ms 的较小值。
                    std::thread::sleep(Duration::from_micros(interval_us.clamp(200, 1_000)));
                }
            });
        match thread {
            Ok(join) => (
                Self {
                    stop,
                    handle: Some(join),
                },
                frame_handle,
            ),
            Err(err) => {
                tracing::error!(error = %err, "渲染线程启动失败");
                (Self { stop, handle: None }, frame_handle)
            }
        }
    }

    /// 请求停止渲染线程。
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// 等待渲染线程退出。
    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.stop();
        self.join();
    }
}

/// 进程内单调时钟的微秒时间戳（§6）。
fn now_us() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_micros() as u64
}

/// 帧节奏（按 vblank 上屏，§3.10）。
#[derive(Debug, Default)]
pub struct FramePacer {
    /// 上一次允许上屏的时刻。
    last_present_us: Option<u64>,
    /// 丢弃帧数。
    dropped: u64,
}

impl FramePacer {
    /// 新建节拍器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 判断一帧是否还能赶上当前的 vsync。
    ///
    /// `now_us` 为单调时钟微秒，`interval_us` 为帧间隔。
    pub fn should_present(&mut self, now_us: u64, interval_us: u64) -> bool {
        match self.last_present_us {
            Some(last) if now_us.saturating_sub(last) < interval_us => {
                self.dropped += 1;
                false
            }
            _ => {
                self.last_present_us = Some(now_us);
                true
            }
        }
    }

    /// 已丢弃帧数。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_proto::PlatformHandle;

    fn frame() -> DecodedFrame {
        DecodedFrame {
            frame_id: 1,
            pts_us: 0,
            capture_ts_us: 0,
            width: 1920,
            height: 1080,
            is_key: true,
            surface: PlatformHandle::NONE,
        }
    }

    #[test]
    fn present_without_backend_reports_e10014() {
        #[cfg(not(feature = "d3d11"))]
        {
            let mut r = D3D11Renderer::new();
            let err = r.present(&frame()).expect_err("必须失败");
            assert_eq!(err.code(), 10014);
        }
        #[cfg(feature = "d3d11")]
        {
            // 没有绑定窗口时不能上屏：交换链还不存在。
            let mut r = D3D11Renderer::new();
            let err = r.present(&frame()).expect_err("必须失败");
            assert_eq!(err.code(), 10012);
        }
    }

    #[test]
    fn pacer_drops_frames_inside_interval() {
        let mut pacer = FramePacer::new();
        assert!(pacer.should_present(0, 16_666));
        assert!(!pacer.should_present(1_000, 16_666));
        assert!(pacer.should_present(20_000, 16_666));
        assert_eq!(pacer.dropped(), 1);
    }

    #[test]
    fn single_slot_hands_latest_frame_to_render_thread() {
        let renderer = D3D11Renderer::new();
        let handle = renderer.frame_handle();
        handle.submit(frame());
        handle.submit(DecodedFrame {
            frame_id: 2,
            ..frame()
        });
        // 渲染线程取走的是最新的一帧。
        let taken = renderer.slot.take();
        assert!(taken.is_some());
        assert_eq!(taken.expect("应当有帧").frame_id, 2);
    }

    #[test]
    fn present_pending_without_frame_reports_false() {
        let mut renderer = D3D11Renderer::new();
        // 没有提交任何帧时，这一拍无事可做。
        let outcome = renderer.present_pending();
        assert!(outcome.is_ok());
        assert!(!outcome.expect("应当返回 false"));
    }
}
