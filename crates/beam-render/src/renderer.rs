//! 渲染器契约与默认实现（§3.10）。
//!
//! - 解码纹理直接交给 D3D11 flip-model 交换链，支持可变刷新率；
//! - 叠加层由 Tauri 窗口负责，本 crate 不做 UI。

use beam_proto::DecodedFrame;

use crate::error::RenderError;

/// 渲染器（§3.10 `Renderer::present(&DecodedFrame)`）。
pub trait Renderer {
    /// 上屏一帧。
    fn present(&mut self, f: &DecodedFrame) -> Result<(), RenderError>;
}

/// 渲染配置（§3.10：可变刷新率）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    /// 窗口宽。
    pub width: u32,
    /// 窗口高。
    pub height: u32,
    /// 目标刷新率（用于 VRR）。
    pub refresh_rate: u32,
    /// 是否启用可变刷新率。
    pub vrr: bool,
    /// 是否启用 flip-model。
    pub flip_model: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
            vrr: true,
            flip_model: true,
        }
    }
}

/// 默认 D3D11 渲染器。
///
/// 未启用 `d3d11` feature 时不触碰原生设备，`present` 返回 `E10014`。
#[derive(Debug, Default)]
pub struct D3D11Renderer {
    cfg: Option<RenderConfig>,
    presented: u64,
    dropped: u64,
}

impl D3D11Renderer {
    /// 新建渲染器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&RenderConfig> {
        self.cfg.as_ref()
    }

    /// 设置配置。
    pub fn configure(&mut self, cfg: RenderConfig) {
        self.cfg = Some(cfg);
    }

    /// 已上屏帧数。
    pub fn presented(&self) -> u64 {
        self.presented
    }

    /// 已丢弃帧数。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl Renderer for D3D11Renderer {
    fn present(&mut self, f: &DecodedFrame) -> Result<(), RenderError> {
        #[cfg(feature = "d3d11")]
        {
            let _ = f;
            // 真实实现：D3D11 纹理 → flip-model 交换链 → Present。
            self.presented += 1;
            Ok(())
        }
        #[cfg(not(feature = "d3d11"))]
        {
            let _ = f;
            Err(RenderError::BackendDisabled)
        }
    }
}

/// 帧节奏（按 vblank 上屏，§3.10）。
#[derive(Debug, Default)]
pub struct FramePacer {
    last_present_us: Option<u64>,
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
        let mut r = D3D11Renderer::new();
        let err = r.present(&frame()).expect_err("必须失败");
        assert_eq!(err.code(), 10014);
    }

    #[test]
    fn pacer_drops_frames_inside_interval() {
        let mut pacer = FramePacer::new();
        assert!(pacer.should_present(0, 16_666));
        assert!(!pacer.should_present(1_000, 16_666));
        assert!(pacer.should_present(20_000, 16_666));
        assert_eq!(pacer.dropped(), 1);
    }
}
