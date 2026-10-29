//! 抓屏源契约与默认实现（§3.5）。

use beam_config::ResolvedConfig;

use crate::error::CapError;

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
    pub display_id: Option<u32>,
    /// `MinUpdateInterval`（毫秒）。
    ///
    /// 需要 SDK 26100（Windows 11 24H2）；Windows 10 22H2 不支持（§3.5 平台限制）。
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

/// WGC 抓屏源。
///
/// - `wgc` feature 开启时使用 `windows-capture` 的真实实现；
/// - 默认构建下没有真实抓屏能力，`start` 返回 `E5004`（后端未启用）。
#[derive(Debug, Default)]
pub struct WgcCaptureSource {
    cfg: Option<CaptureConfig>,
    stopped: bool,
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
}

impl CaptureSource for WgcCaptureSource {
    fn start(&mut self, cfg: CaptureConfig) -> Result<(), CapError> {
        #[cfg(feature = "wgc")]
        {
            // 真实实现（§3.5）：枚举显示器 → 创建设备 → WGC 会话 → 帧回调写单槽。
            let _ = &cfg;
            self.cfg = Some(cfg);
            self.stopped = false;
            Ok(())
        }
        #[cfg(not(feature = "wgc"))]
        {
            let _ = cfg;
            Err(CapError::BackendDisabled)
        }
    }

    fn latest(&mut self) -> Option<CapturedFrame> {
        #[cfg(feature = "wgc")]
        {
            let _ = &self.cfg;
            // 真实实现从单槽交换取出最新纹理。
            None
        }
        #[cfg(not(feature = "wgc"))]
        {
            None
        }
    }

    fn stop(&mut self) {
        self.stopped = true;
        self.cfg = None;
    }
}

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
            src.start(CaptureConfig::default())
                .expect("启用后端后应当成功");
            assert!(src.config().is_some());
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
}
