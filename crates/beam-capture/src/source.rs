//! 抓屏源契约与默认实现（§3.5）。
//!
//! 抓屏后端的实现在 `wgc` feature 后面：
//!
//! - [`WgcCaptureSource`] 是对外唯一的抓屏源，按 [`CaptureConfig::backend`]
//!   在 WGC（主）与 DXGI（回退）之间选择；
//! - 两个后端都遵守 §3.5 的线程模型：抓屏侧只往「最新帧」单槽里写，
//!   编码节拍线程用 [`CaptureSource::latest`] 非阻塞取最新帧。

use std::fmt;
use std::sync::OnceLock;
use std::time::Instant;

use beam_config::ResolvedConfig;

use crate::error::CapError;
use crate::slot::FrameSlot;

#[cfg(feature = "wgc")]
use crate::{dxgi, wgc};

/// 抓屏后端（§3.5 `backend=auto|wgc|dxgi`）。
///
/// **单一真值来源**：枚举本体定义在 beam-config（配置模型、校验与持久化的归属地），
/// 本 crate 直接重新导出，不再自带一份平行定义。
pub use beam_config::CaptureBackend;

/// GPU 纹理句柄。
///
/// Windows 上是 D3D11 纹理（§6：COM 指针引用计数，capture→encode 同一设备零拷贝）。
#[cfg(windows)]
pub type Texture = windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

/// 非 Windows 平台的占位纹理。
///
/// §7 的 CI 在 Linux 上只编译纯逻辑 crate；这里给出可编译的占位类型，
/// 保证 `beam-capture` 在任何平台都能通过 `cargo check`。
#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Texture {
    /// 占位宽度。
    pub width: u32,
    /// 占位高度。
    pub height: u32,
}

/// 抓屏配置（§3.5）。
///
/// **单一真值来源**：`backend` / `display_id` / `min_update_interval` 三项
/// 由 beam-config 的 [`ResolvedConfig`] 派生（见 `From<&ResolvedConfig>`），
/// 本 crate 不再独立定义这些配置项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// 抓屏后端。
    pub backend: CaptureBackend,
    /// 目标显示器 id；`None` 表示主显示器。
    ///
    /// 编号口径是 **1 基**（与 Windows 的 `\\.\DISPLAYn` 一致）：
    /// `Some(1)` 是枚举到的第一个显示器，`Some(0)` 非法（会被判为 `E5001`）。
    pub display_id: Option<u32>,
    /// `MinUpdateInterval`（毫秒）。
    ///
    /// 需要 SDK 26100（Windows 11 24H2）；Windows 10 22H2 不支持（§3.5 平台限制）。
    /// 不支持时抓屏层会**明确降级**为系统默认更新间隔，并通过
    /// [`CaptureDegradation::MinUpdateIntervalUnsupported`] 如实反映，不会假装设置成功。
    pub min_update_interval: Option<u32>,
    /// 目标帧率（抓屏节拍）。
    pub target_fps: u32,
}

impl From<&ResolvedConfig> for CaptureConfig {
    /// 从配置层（§3.14 合并后的结果）派生抓屏配置。
    ///
    /// `backend` 取强类型枚举（非法值由 beam-config 的 `validate` 拦下，
    /// 这里兜底回退到 `auto`）；`target_fps` 复用视频层的目标帧率。
    fn from(cfg: &ResolvedConfig) -> Self {
        Self {
            backend: cfg.capture.backend_kind(),
            display_id: cfg.capture.display_id,
            min_update_interval: cfg.capture.min_update_interval,
            target_fps: cfg.video.fps,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            backend: CaptureBackend::Auto,
            display_id: None,
            min_update_interval: None,
            target_fps: 60,
        }
    }
}

/// 光标模式（§3.5「光标单独通道」）。
///
/// WGC 把光标合成进画面；要让光标走**独立通道**，就必须在抓屏侧关闭光标合成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorMode {
    /// 光标合成进画面（WGC 的系统默认行为）。
    #[default]
    Composited,
    /// 光标不合成进画面，留给独立的光标通道单独传输。
    Separate,
}

/// 抓屏链路的降级记录（§3.5 平台限制）。
///
/// 降级**不是错误**：抓屏仍然可用，但某些能力没有按配置生效，
/// 这里如实记录，避免上层误以为「设置成功」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureDegradation {
    /// `min_update_interval` 未生效：当前系统不支持 `MinUpdateInterval`。
    ///
    /// §3.5 平台限制：该能力需要 Windows 11 24H2（SDK 26100），
    /// Windows 10 22H2 上抓屏上限约 60fps。
    MinUpdateIntervalUnsupported {
        /// 用户请求的最小更新间隔（毫秒）。
        requested_ms: u32,
    },
    /// 光标开关不可用：已回退为「光标合成进画面」。
    CursorCaptureUnsupported,
}

/// 抓屏帧（§3.5 `CapturedFrame`）。
#[derive(Debug)]
pub struct CapturedFrame {
    /// GPU 纹理（不拷到 CPU，§3.5 边界）。
    pub tex: Texture,
    /// 抓屏时刻，微秒（§6：主机抓屏时刻即 `capture_ts`）。
    pub ts_us: u64,
    /// 与上一帧相比是否变化。
    pub dirty: bool,
}

/// 抓屏源（§3.5 `CaptureSource`）。
pub trait CaptureSource: Send {
    /// 开始抓屏。
    fn start(&mut self, cfg: CaptureConfig) -> Result<(), CapError>;
    /// 取最新一帧；没有新帧返回 `None`（非阻塞）。
    fn latest(&mut self) -> Option<CapturedFrame>;
    /// 停止抓屏。
    fn stop(&mut self);
}

/// 已发布到单槽里的一帧。
///
/// 只有 `wgc` feature（真实抓屏后端）才会用到它。
#[cfg_attr(not(feature = "wgc"), allow(dead_code))]
pub(crate) struct PublishedFrame {
    /// GPU 纹理。
    pub(crate) tex: Texture,
    /// 抓屏时刻，微秒。
    pub(crate) ts_us: u64,
}

/// 进程内单调时钟的微秒时间戳（§6：所有时间戳都是单调时钟微秒）。
///
/// 零点取进程内第一次调用该函数的时刻，因此不同后端（WGC / DXGI）
/// 产出的时间戳处在**同一个时钟域**里，可以直接做端到端延迟计算。
pub(crate) fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_micros() as u64
}

/// 按 `display_id` 选出目标显示器（WGC 与 DXGI 两个后端共用）。
///
/// `display_id` 是 **1 基**编号（与 Windows 的 `\\.\DISPLAYn` 一致）：
/// `None` 表示主显示器；编号不存在时按 `E5001` 回报。
#[cfg(feature = "wgc")]
pub(crate) fn select_monitor(
    cfg: &CaptureConfig,
) -> Result<windows_capture::monitor::Monitor, CapError> {
    match cfg.display_id {
        Some(id) => windows_capture::monitor::Monitor::from_index(id as usize).map_err(|err| {
            tracing::error!(display_id = id, error = %err, "找不到目标显示器");
            CapError::NoDisplay
        }),
        None => windows_capture::monitor::Monitor::primary().map_err(|err| {
            tracing::error!(error = %err, "找不到主显示器");
            CapError::NoDisplay
        }),
    }
}

/// 当前生效的后端。
enum Backend {
    /// WGC（主）。
    #[cfg(feature = "wgc")]
    Wgc(wgc::WgcBackend),
    /// DXGI 桌面复制（回退）。
    #[cfg(feature = "wgc")]
    Dxgi(dxgi::DxgiBackend),
}

/// 抓屏源（§3.5）。
///
/// 默认实现，承载 WGC / DXGI 两个后端的选择逻辑：
///
/// - `backend=auto`：先试 WGC，遇到 `E5002`（独占全屏）按 §3.5 切到 DXGI；
/// - `backend=wgc`：只用 WGC，遇到独占全屏直接回报 `E5002`；
/// - `backend=dxgi`：只用 DXGI 桌面复制。
///
/// 未开启 `wgc` feature 时不触碰任何原生 API，`start` 返回 `E5004`
/// （**非文档码位**，只表示「后端未编译进来」）。
#[derive(Default)]
pub struct WgcCaptureSource {
    /// 当前配置（已启动时有效）。
    cfg: Option<CaptureConfig>,
    /// 是否已停止。
    stopped: bool,
    /// 光标模式。
    cursor_mode: CursorMode,
    /// 降级记录。
    degradations: Vec<CaptureDegradation>,
    /// 当前生效的后端。
    backend: Option<Backend>,
}

impl fmt::Debug for WgcCaptureSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WgcCaptureSource")
            .field("backend", &self.backend_kind())
            .field("cursor_mode", &self.cursor_mode)
            .field("degradations", &self.degradations)
            .field("stopped", &self.stopped)
            .finish()
    }
}

impl WgcCaptureSource {
    /// 新建抓屏源。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前配置（已启动时有效）。
    pub fn config(&self) -> Option<&CaptureConfig> {
        self.cfg.as_ref()
    }

    /// 当前是否处于停止状态。
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// 设置光标模式（§3.5「光标单独通道」）。
    ///
    /// 必须在 [`CaptureSource::start`] 之前设置；启动之后再改不会生效。
    pub fn set_cursor_mode(&mut self, mode: CursorMode) {
        self.cursor_mode = mode;
    }

    /// 当前光标模式。
    pub fn cursor_mode(&self) -> CursorMode {
        self.cursor_mode
    }

    /// 抓屏链路的降级记录（§3.5 平台限制）。
    ///
    /// 空表示所有请求的能力都按配置生效了。
    pub fn degradations(&self) -> &[CaptureDegradation] {
        &self.degradations
    }

    /// 当前生效的后端名（`"wgc"` / `"dxgi"` / `None`）。
    pub fn backend_kind(&self) -> Option<&'static str> {
        match &self.backend {
            #[cfg(feature = "wgc")]
            Some(Backend::Wgc(_)) => Some("wgc"),
            #[cfg(feature = "wgc")]
            Some(Backend::Dxgi(_)) => Some("dxgi"),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// 抓屏是否还在正常工作。
    ///
    /// WGC 后端有独立的捕获线程：如果它异常退出（WGC 被系统关闭、
    /// 设备丢失等），这里会返回 `false`，上层可以据此重建抓屏管线（`E5003`）。
    /// DXGI 后端是拉取式的（没有独立线程），只要后端还在就算在工作。
    pub fn capture_running(&self) -> bool {
        #[cfg(feature = "wgc")]
        {
            match &self.backend {
                Some(Backend::Wgc(backend)) => !backend.is_finished(),
                Some(Backend::Dxgi(_)) => true,
                None => false,
            }
        }
        #[cfg(not(feature = "wgc"))]
        {
            false
        }
    }
}

impl CaptureSource for WgcCaptureSource {
    fn start(&mut self, cfg: CaptureConfig) -> Result<(), CapError> {
        // 先把单调时钟的零点定下来：这样第一帧的 `ts_us` 就是「抓屏启动后过了多少微秒」，
        // 而不是「进程内某个更早的时刻」。
        let _ = now_us();
        #[cfg(feature = "wgc")]
        {
            let mut degradations = Vec::new();
            let backend = match cfg.backend {
                CaptureBackend::Auto => {
                    match wgc::WgcBackend::start(&cfg, self.cursor_mode, &mut degradations) {
                        Ok(backend) => Backend::Wgc(backend),
                        // §3.5 E5002：独占全屏无法抓取 → 切 DXGI。
                        Err(CapError::ExclusiveFullscreen) => {
                            tracing::info!("WGC 被独占全屏挡住,按 §3.5 切到 DXGI 回退");
                            Backend::Dxgi(dxgi::DxgiBackend::start(&cfg)?)
                        }
                        Err(err) => return Err(err),
                    }
                }
                CaptureBackend::Wgc => Backend::Wgc(wgc::WgcBackend::start(
                    &cfg,
                    self.cursor_mode,
                    &mut degradations,
                )?),
                CaptureBackend::Dxgi => Backend::Dxgi(dxgi::DxgiBackend::start(&cfg)?),
            };
            self.backend = Some(backend);
            self.degradations = degradations;
            self.cfg = Some(cfg);
            self.stopped = false;
            Ok(())
        }
        #[cfg(not(feature = "wgc"))]
        {
            let _ = cfg;
            // E5004 不是 §3.5 定义的码位：这里表示「后端未编译进来」。
            Err(CapError::BackendDisabled)
        }
    }

    fn latest(&mut self) -> Option<CapturedFrame> {
        #[cfg(feature = "wgc")]
        {
            match &mut self.backend {
                Some(Backend::Wgc(backend)) => backend.latest(),
                Some(Backend::Dxgi(backend)) => backend.latest(),
                None => None,
            }
        }
        #[cfg(not(feature = "wgc"))]
        {
            None
        }
    }

    fn stop(&mut self) {
        #[cfg(feature = "wgc")]
        {
            if let Some(backend) = self.backend.take() {
                match backend {
                    Backend::Wgc(mut backend) => backend.stop(),
                    Backend::Dxgi(mut backend) => backend.stop(),
                }
            }
        }
        self.stopped = true;
        self.cfg = None;
    }
}

/// 抓屏后端共用的单槽（供 `wgc` / `dxgi` 两个后端复用）。
#[cfg_attr(not(feature = "wgc"), allow(dead_code))]
pub(crate) type FrameChannel = FrameSlot<PublishedFrame>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backend_is_auto() {
        assert_eq!(CaptureConfig::default().backend, CaptureBackend::Auto);
        assert_eq!(CaptureConfig::default().target_fps, 60);
        assert_eq!(CaptureConfig::default().min_update_interval, None);
    }

    #[test]
    fn start_without_backend_feature_reports_e5004() {
        let mut src = WgcCaptureSource::new();
        #[cfg(not(feature = "wgc"))]
        {
            let err = src
                .start(CaptureConfig::default())
                .expect_err("未启用后端必须失败");
            assert_eq!(err.code(), 5004);
        }
        #[cfg(feature = "wgc")]
        {
            // 真实后端需要真实显示器：这里只验证接口形状，不假设抓屏一定成功。
            let _ = src.start(CaptureConfig::default());
            src.stop();
        }
        assert!(src.latest().is_none());
    }

    #[test]
    fn stop_resets_state() {
        let mut src = WgcCaptureSource::new();
        src.stop();
        assert!(src.is_stopped());
        assert!(src.config().is_none());
    }

    #[test]
    fn capture_config_is_derived_from_resolved_config() {
        let mut cfg = ResolvedConfig::default();
        cfg.capture.backend = "wgc".to_string();
        cfg.capture.display_id = Some(2);
        cfg.capture.min_update_interval = Some(8);
        cfg.video.fps = 120;

        let cap = CaptureConfig::from(&cfg);
        assert_eq!(cap.backend, CaptureBackend::Wgc);
        assert_eq!(cap.display_id, Some(2));
        assert_eq!(cap.min_update_interval, Some(8));
        // 抓屏节拍跟随视频层的目标帧率。
        assert_eq!(cap.target_fps, 120);
    }

    #[test]
    fn capture_config_from_default_resolved_uses_builtin_defaults() {
        let cap = CaptureConfig::from(&ResolvedConfig::default());
        assert_eq!(cap.backend, CaptureBackend::Auto);
        assert_eq!(cap.display_id, None);
        assert_eq!(cap.min_update_interval, None);
        assert_eq!(cap.target_fps, 60);
    }

    #[test]
    fn capture_config_derivation_falls_back_to_auto_on_bad_backend() {
        // 非法后端应当已被 beam-config 的 validate 拦下；
        // 这里验证派生侧的兜底不会 panic。
        let mut cfg = ResolvedConfig::default();
        cfg.capture.backend = "miracast".to_string();
        assert_eq!(CaptureConfig::from(&cfg).backend, CaptureBackend::Auto);
    }

    #[test]
    fn cursor_mode_defaults_to_composited() {
        let src = WgcCaptureSource::new();
        assert_eq!(src.cursor_mode(), CursorMode::Composited);
        let mut src = src;
        src.set_cursor_mode(CursorMode::Separate);
        assert_eq!(src.cursor_mode(), CursorMode::Separate);
    }

    #[test]
    fn monotonic_clock_is_monotonic() {
        let a = now_us();
        let b = now_us();
        assert!(b >= a);
    }
}
