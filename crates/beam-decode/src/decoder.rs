//! 解码器契约与默认实现（§3.10）。
//!
//! §3.10：ffmpeg D3D11VA 硬解 HEVC / H.264（/AV1 v2）；
//! 解码纹理直接交给 D3D11 flip-model 交换链，支持可变刷新率；叠加层由 Tauri 窗口负责。
//!
//! §4 分层说明：§3.10 的签名是 `VideoDecoder::decode(&mut self, f: AssembledFrame)`，
//! 而 `AssembledFrame` 定义在 §3.2（beam-net）。§4 规定 L1 之间只有 net→pair 可以横向依赖，
//! 因此该结构放在 L0 的 beam-proto 里，由 beam-net 重导出。
//!
//! ## 验证状态
//!
//! **「D3D11VA 硬解」已在本机真机验证**（§9 M-1 验证点③的解码侧）：
//! 见 `examples/d3d11va_decode_smoke.rs`——喂进 NVENC 真实产出的 HEVC 码流，
//! 用**注入的 D3D11 设备**解出 1920×1080 的画面，并核对渲染目标句柄。
//! 探测入口见 `examples/probe_decoders.rs`。

use beam_proto::{AssembledFrame, CodecKind, DecodedFrame, PlatformHandle};

use crate::error::DecError;
#[cfg(feature = "ffmpeg")]
use crate::slot::DecodedOutput;

/// 解码配置（§3.10）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeConfig {
    /// 目标编解码器。
    pub codec: CodecKind,
    /// 是否启用硬解（D3D11VA）。
    pub hardware: bool,
    /// 是否启用低延迟解码。
    pub low_latency: bool,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            codec: CodecKind::Hevc,
            hardware: true,
            low_latency: true,
        }
    }
}

/// 视频解码器（§3.10 `VideoDecoder`）。
pub trait VideoDecoder {
    /// 解码一帧。
    fn decode(&mut self, f: AssembledFrame) -> Result<DecodedFrame, DecError>;
    /// 请求解码器重建（设备丢失后）。
    fn reset(&mut self) -> Result<(), DecError>;
}

/// 当前构建是否支持该编解码器（§7：AV1 是 v2 能力，默认关闭）。
pub fn supports_codec(codec: CodecKind) -> bool {
    match codec {
        CodecKind::Av1 => cfg!(feature = "av1"),
        CodecKind::H264 | CodecKind::Hevc => true,
    }
}

/// 解码输出的渲染目标（D3D11 纹理）的保活引用。
///
/// §3.10 要求「解码纹理直接交给 D3D11 flip-model 交换链」：
/// 纹理必须在渲染线程上屏之前一直存活，因此这里持有一份 COM 引用。
#[derive(Debug, Clone)]
pub struct DecodedSurface {
    /// 平台指针（Windows 上是 `ID3D11Texture2D *`）。
    raw: usize,
    /// 保活用的 COM 引用（Windows 上是 `ID3D11Texture2D`）。
    #[cfg(windows)]
    _guard: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
}

impl DecodedSurface {
    /// 用一个 Windows D3D11 纹理建立保活引用。
    #[cfg(windows)]
    #[allow(
        unsafe_code,
        reason = "需要从裸指针借用 COM 接口并 AddRef，以延长纹理寿命"
    )]
    pub fn from_texture(texture: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D) -> Self {
        use windows::core::Interface;
        let raw = texture.as_raw() as usize;
        Self {
            raw,
            _guard: Some(texture),
        }
    }

    /// 非 Windows 平台的占位构造。
    #[cfg(not(windows))]
    pub fn from_raw(raw: usize) -> Self {
        Self { raw }
    }

    /// 渲染目标句柄（交给渲染层）。
    pub fn handle(&self) -> PlatformHandle {
        PlatformHandle { raw: self.raw }
    }
}

/// 默认 D3D11VA 解码器。
///
/// 未启用 `ffmpeg` feature 时不触碰原生库，`decode` 返回 `E10004`；
/// 启用了但本机没有可用硬解时返回 `E10001`。
pub struct D3D11VaDecoder {
    cfg: Option<DecodeConfig>,
    #[cfg(feature = "ffmpeg")]
    backend: Option<crate::ffmpeg::FfmpegD3D11VaDecoder>,
    /// 上一帧输出的纹理保活引用。
    ///
    /// `VideoDecoder::decode` 只返回 [`DecodedFrame`]（协议层不带 COM 指针），
    /// 因此纹理由解码器自己保活到下一帧为止。
    #[cfg(feature = "ffmpeg")]
    surface: Option<DecodedSurface>,
}

impl std::fmt::Debug for D3D11VaDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("D3D11VaDecoder")
            .field("cfg", &self.cfg)
            .finish()
    }
}

impl D3D11VaDecoder {
    /// 新建解码器（尚未建立原生解码上下文）。
    pub fn new() -> Self {
        Self {
            cfg: None,
            #[cfg(feature = "ffmpeg")]
            backend: None,
            #[cfg(feature = "ffmpeg")]
            surface: None,
        }
    }

    /// 按配置建立解码上下文（**自建设备**）。
    ///
    /// ⚠️ **生产路径请用 [`Self::open_with_device`]**：自建的 D3D11 设备与渲染层的
    /// 交换链不在同一个设备上，解码纹理无法零拷贝地交给渲染层
    /// （D3D11 禁止跨设备拷贝）。
    ///
    /// - 后端未启用（无 `ffmpeg` feature）→ `E10004`。
    /// - 本机没有可用硬解 → `E10001`。
    #[cfg(feature = "ffmpeg")]
    pub fn open(cfg: DecodeConfig) -> Result<Self, DecError> {
        let backend = crate::ffmpeg::FfmpegD3D11VaDecoder::open(cfg.codec)?;
        Ok(Self {
            cfg: Some(cfg),
            backend: Some(backend),
            surface: None,
        })
    }

    /// 按配置建立解码上下文，**复用渲染层的 D3D11 设备**。
    ///
    /// ## 跨 crate 设备契约（硬约束）
    ///
    /// D3D11 **不允许跨设备拷贝**。渲染层的 `D3D11Renderer::device()` 返回的是
    /// 渲染层自己创建的设备，交换链与后备缓冲都在它上面；解码出来的纹理必须
    /// 创建在**同一个设备**上，否则渲染层无法把它拷进后备缓冲，帧链路是断的。
    ///
    /// §4 规定 L1 之间不能横向依赖，beam-decode 因此不能直接引用 beam-render 的类型，
    /// 契约只能通过 `ID3D11Device` 这个平台类型来表达；由调用方（beam-client）
    /// 把 `D3D11Renderer::device()` 的结果传进来。
    ///
    /// - 后端未启用（无 `ffmpeg` feature）→ `E10004`。
    /// - 本机没有可用硬解 → `E10001`。
    #[cfg(feature = "ffmpeg")]
    pub fn open_with_device(
        cfg: DecodeConfig,
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    ) -> Result<Self, DecError> {
        let backend = crate::ffmpeg::FfmpegD3D11VaDecoder::open_on_device(cfg.codec, device)?;
        Ok(Self {
            cfg: Some(cfg),
            backend: Some(backend),
            surface: None,
        })
    }

    /// 按配置建立解码上下文。
    ///
    /// 无 `ffmpeg` feature 时直接返回 `E10004`（后端未启用）。
    #[cfg(not(feature = "ffmpeg"))]
    pub fn open(cfg: DecodeConfig) -> Result<Self, DecError> {
        let _ = cfg;
        Err(DecError::BackendDisabled)
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&DecodeConfig> {
        self.cfg.as_ref()
    }

    /// 解码纹理所在的 D3D11 设备（**必须**与渲染层的是同一个）。
    ///
    /// 供渲染层做运行期设备一致性核对。
    #[cfg(feature = "ffmpeg")]
    pub fn device(&self) -> Option<&windows::Win32::Graphics::Direct3D11::ID3D11Device> {
        self.backend.as_ref().map(|b| b.device())
    }

    /// 设置配置（启动解码器前调用）。
    pub fn configure(&mut self, cfg: DecodeConfig) {
        self.cfg = Some(cfg);
    }

    /// 解码一帧，并同时返回渲染目标的保活引用。
    ///
    /// 供「解码线程 → 单槽 → 渲染线程」的单槽流水线使用
    /// （见 [`crate::slot::FrameSlot`]）。
    #[cfg(feature = "ffmpeg")]
    pub fn decode_output(&mut self, f: AssembledFrame) -> Result<DecodedOutput, DecError> {
        let backend = self.backend.as_mut().ok_or(DecError::DecoderUnavailable)?;
        // §3.13「解码」阶段：D3D11VA 硬解一帧的真实耗时。
        let _decode_span = perf_span!("decode");
        let out = backend.decode(&f)?;
        self.surface = out.surface.clone();
        Ok(out)
    }
}

impl Default for D3D11VaDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoDecoder for D3D11VaDecoder {
    fn decode(&mut self, f: AssembledFrame) -> Result<DecodedFrame, DecError> {
        #[cfg(feature = "ffmpeg")]
        {
            let out = self.decode_output(f)?;
            Ok(out.frame)
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            let _ = f;
            Err(DecError::BackendDisabled)
        }
    }

    fn reset(&mut self) -> Result<(), DecError> {
        #[cfg(feature = "ffmpeg")]
        {
            // 设备丢失后重建解码器与 D3D11 上下文。
            let codec = self
                .cfg
                .as_ref()
                .map(|c| c.codec)
                .unwrap_or(CodecKind::Hevc);
            self.backend = None;
            self.surface = None;
            self.backend = Some(crate::ffmpeg::FfmpegD3D11VaDecoder::open(codec)?);
            Ok(())
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            Err(DecError::BackendDisabled)
        }
    }
}

/// 由帧号与平台句柄构造一个解码结果（供渲染层与测试使用）。
pub fn make_decoded_frame(
    frame_id: u32,
    pts_us: u64,
    capture_ts_us: u64,
    width: u32,
    height: u32,
    is_key: bool,
    surface: PlatformHandle,
) -> DecodedFrame {
    DecodedFrame {
        frame_id,
        pts_us,
        capture_ts_us,
        width,
        height,
        is_key,
        surface,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn frame() -> AssembledFrame {
        AssembledFrame {
            frame_id: 7,
            pts_us: 100,
            capture_ts_us: 90,
            is_key: true,
            payload: Bytes::from_static(b"nal"),
        }
    }

    #[test]
    fn decode_without_backend_reports_e10004() {
        let mut dec = D3D11VaDecoder::new();
        let err = dec.decode(frame()).expect_err("必须失败");
        #[cfg(not(feature = "ffmpeg"))]
        assert_eq!(err.code(), 10004);
        // 有后端但没建立解码上下文 → E10001（无可用硬解）。
        #[cfg(feature = "ffmpeg")]
        assert_eq!(err.code(), 10001);
    }

    #[test]
    fn decoded_frame_carries_timestamps() {
        let f = make_decoded_frame(1, 2, 3, 1920, 1080, true, PlatformHandle::NONE);
        assert_eq!(f.frame_id, 1);
        assert_eq!(f.pts_us, 2);
        assert_eq!(f.capture_ts_us, 3);
        assert!(f.surface.is_none());
    }

    #[test]
    fn av1_is_gated_by_feature() {
        assert_eq!(
            supports_codec(CodecKind::Av1),
            cfg!(feature = "av1"),
            "AV1 必须由 av1 feature 门控（§7）"
        );
        assert!(supports_codec(CodecKind::Hevc));
    }
}
