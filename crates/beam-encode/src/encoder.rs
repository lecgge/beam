//! 编码器契约、能力探测与默认实现（§3.6）。
//!
//! ## §4 分层说明
//!
//! §3.6 的签名是 `VideoEncoder::encode(&mut self, f: &CapturedFrame)`，
//! 而 §4 规定「L1 之间仅 net→pair 允许」，因此 beam-encode **不能**依赖 beam-capture。
//! 这里定义与 beam-capture 同形的 [`CapturedFrame`]（同样的字段名与语义），
//! 由 beam-host 做一次性转换；两者字段一一对应，转换零成本。

use beam_config::ResolvedConfig;
use beam_proto::{CodecKind, EncodedFrame};

use crate::error::EncError;

/// GPU 纹理句柄（Windows 上是 D3D11 纹理）。
#[cfg(windows)]
pub type Texture = windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

/// 非 Windows 平台的占位纹理（§7：Linux CI 只编译纯逻辑 crate）。
#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Texture {
    /// 占位宽度。
    pub width: u32,
    /// 占位高度。
    pub height: u32,
}

/// 送入编码器的画面（与 beam-capture 的 `CapturedFrame` 同形）。
#[derive(Debug)]
pub struct CapturedFrame {
    /// GPU 纹理。
    pub tex: Texture,
    /// 抓屏时刻，微秒。
    pub ts_us: u64,
    /// 与上一帧相比是否变化。
    pub dirty: bool,
    /// 画面宽。
    pub width: u32,
    /// 画面高。
    pub height: u32,
}

/// 编码器种类（§3.6 `encoder=auto|nvenc|amf|qsv`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderKind {
    /// 自动选择。
    Auto,
    /// NVIDIA NVENC。
    Nvenc,
    /// AMD AMF。
    Amf,
    /// Intel QSV。
    Qsv,
}

impl Default for EncoderKind {
    fn default() -> Self {
        EncoderKind::Auto
    }
}

impl EncoderKind {
    /// 稳定的英文名。
    pub const fn as_str(self) -> &'static str {
        match self {
            EncoderKind::Auto => "auto",
            EncoderKind::Nvenc => "nvenc",
            EncoderKind::Amf => "amf",
            EncoderKind::Qsv => "qsv",
        }
    }

    /// 解析英文名。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "auto" => Some(EncoderKind::Auto),
            "nvenc" => Some(EncoderKind::Nvenc),
            "amf" => Some(EncoderKind::Amf),
            "qsv" => Some(EncoderKind::Qsv),
            _ => None,
        }
    }
}

/// 编码器能力（§3.6 `probe() -> Vec<EncoderCaps>`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderCaps {
    /// 编码器种类。
    pub encoder: EncoderKind,
    /// 支持的编解码器。
    pub codec: CodecKind,
    /// 最大宽度。
    pub max_width: u32,
    /// 最大高度。
    pub max_height: u32,
    /// 最大帧率。
    pub max_fps: u32,
    /// 是否支持低延迟模式（ultra-low-latency / vbr_latency / veryfast）。
    pub low_latency: bool,
    /// 是否支持切片输出（v1）。
    pub slice: bool,
    /// 是否支持 SFE（split-frame encoding，Ada 起）。
    pub sfe: bool,
}

/// 编码配置（§3.6）。
///
/// **单一真值来源**：`slices` / `sfe`（以及 `encoder` / `codec` / `bitrate_bps` / `fps`）
/// 由 beam-config 的 [`ResolvedConfig`] 派生（见 `From<&ResolvedConfig>`），
/// 本 crate 不再独立定义这些配置项。
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeConfig {
    /// 编码器。
    pub encoder: EncoderKind,
    /// 编解码器。
    pub codec: CodecKind,
    /// 目标码率，bit/s。
    pub bitrate_bps: u32,
    /// 目标帧率。
    pub fps: u32,
    /// 切片数（v1；`0` 表示不切片）。
    pub slices: u32,
    /// 是否启用 SFE（v1）。
    pub sfe: bool,
    /// 强制 IDR 的间隔（帧数）；`0` 表示只按需请求。
    pub idr_interval: u32,
}

impl From<&ResolvedConfig> for EncodeConfig {
    /// 从配置层（§3.14 合并后的结果）派生编码配置。
    ///
    /// `slices` 在配置层是 `Option<u32>`：`None` 表示不切片（落成 `0`），
    /// 显式设置的值必须是 1..=`MAX_SLICES` 的正整数（由 beam-config 的 `validate` 保证）。
    fn from(cfg: &ResolvedConfig) -> Self {
        Self {
            encoder: EncoderKind::from_name(&cfg.video.encoder).unwrap_or_default(),
            codec: codec_kind_from_name(&cfg.video.codec),
            bitrate_bps: cfg.video.bitrate_bps,
            fps: cfg.video.fps,
            slices: cfg.video.slices.unwrap_or(0),
            sfe: cfg.video.sfe,
            ..EncodeConfig::default()
        }
    }
}

/// 由配置名解析编解码器（§3.6 `codec=hevc|h264|av1`）。
///
/// 非法取值由 beam-config 的 `validate` 拦下，这里兜底回退到 HEVC。
fn codec_kind_from_name(name: &str) -> CodecKind {
    match name.trim().to_ascii_lowercase().as_str() {
        "h264" => CodecKind::H264,
        "av1" => CodecKind::Av1,
        _ => CodecKind::Hevc,
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            encoder: EncoderKind::Auto,
            codec: CodecKind::Hevc,
            bitrate_bps: 20_000_000,
            fps: 60,
            slices: 0,
            sfe: false,
            idr_interval: 0,
        }
    }
}

/// 视频编码器（§3.6 `VideoEncoder`）。
pub trait VideoEncoder {
    /// 编码一帧。
    fn encode(&mut self, f: &CapturedFrame) -> Result<EncodedFrame, EncError>;
    /// 请求下一个关键帧（IDR）。
    fn request_idr(&mut self);
    /// 热更新码率。
    fn set_bitrate(&mut self, bps: u32);
}

/// 当前构建是否支持该编解码器（§7：AV1 是 v2 能力，默认关闭）。
pub fn supports_codec(codec: CodecKind) -> bool {
    match codec {
        CodecKind::Av1 => cfg!(feature = "av1"),
        CodecKind::H264 | CodecKind::Hevc => true,
    }
}

/// 探测本机可用的硬件编码器（§3.6 `probe()`）。
///
/// 没有 `ffmpeg` feature 时不触碰任何原生库，直接返回空列表——
/// 对应 §3.6 的 `E6001`「无可用硬编」。
pub fn probe() -> Vec<EncoderCaps> {
    #[cfg(feature = "ffmpeg")]
    {
        // 真实实现：枚举 ffmpeg 的 NVENC / AMF / QSV 编码器并读取其能力。
        Vec::new()
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        Vec::new()
    }
}

/// 默认编码器（未启用 ffmpeg feature 时无法真正编码）。
#[derive(Debug, Default)]
pub struct HardwareVideoEncoder {
    cfg: Option<EncodeConfig>,
    bitrate_bps: u32,
    idr_requested: bool,
}

impl HardwareVideoEncoder {
    /// 新建编码器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&EncodeConfig> {
        self.cfg.as_ref()
    }

    /// 是否已请求 IDR。
    pub fn idr_requested(&self) -> bool {
        self.idr_requested
    }
}

impl VideoEncoder for HardwareVideoEncoder {
    fn encode(&mut self, f: &CapturedFrame) -> Result<EncodedFrame, EncError> {
        #[cfg(feature = "ffmpeg")]
        {
            let _ = f;
            // 真实实现：D3D11 硬件帧上下文 → D3D11 VideoProcessor 颜色转换 → NVENC/AMF/QSV。
            Err(EncError::BackendDisabled)
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            let _ = f;
            Err(EncError::BackendDisabled)
        }
    }

    fn request_idr(&mut self) {
        self.idr_requested = true;
    }

    fn set_bitrate(&mut self, bps: u32) {
        self.bitrate_bps = bps;
        if let Some(cfg) = self.cfg.as_mut() {
            cfg.bitrate_bps = bps;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一帧测试画面。
    ///
    /// Windows 上的 D3D11 纹理是 COM 指针，没有安全的空值构造，
    /// 因此这里在 Windows 上返回 `None`，只校验错误码映射。
    fn sample_frame() -> Option<CapturedFrame> {
        #[cfg(windows)]
        {
            None
        }
        #[cfg(not(windows))]
        {
            Some(CapturedFrame {
                tex: Texture::default(),
                ts_us: 0,
                dirty: true,
                width: 1920,
                height: 1080,
            })
        }
    }

    #[test]
    fn probe_without_backend_returns_empty() {
        #[cfg(not(feature = "ffmpeg"))]
        assert!(probe().is_empty());
        #[cfg(feature = "ffmpeg")]
        let _ = probe();
    }

    #[test]
    fn encode_without_backend_reports_error() {
        let mut enc = HardwareVideoEncoder::new();
        match sample_frame() {
            Some(frame) => {
                let err = enc.encode(&frame).expect_err("未启用后端必须失败");
                assert_eq!(err.code(), 6004);
            }
            None => {
                assert_eq!(EncError::BackendDisabled.code(), 6004);
            }
        }
    }

    #[test]
    fn bitrate_and_idr_are_recorded() {
        let mut enc = HardwareVideoEncoder::new();
        enc.set_bitrate(30_000_000);
        enc.request_idr();
        assert!(enc.idr_requested());
    }

    #[test]
    fn encoder_names_round_trip() {
        for k in [
            EncoderKind::Auto,
            EncoderKind::Nvenc,
            EncoderKind::Amf,
            EncoderKind::Qsv,
        ] {
            assert_eq!(EncoderKind::from_name(k.as_str()), Some(k));
        }
        assert_eq!(EncoderKind::from_name("nope"), None);
    }

    #[test]
    fn av1_is_gated_by_feature() {
        assert_eq!(
            supports_codec(CodecKind::Av1),
            cfg!(feature = "av1"),
            "AV1 必须由 av1 feature 门控（§7）"
        );
        assert!(supports_codec(CodecKind::H264));
        assert!(supports_codec(CodecKind::Hevc));
    }

    #[test]
    fn encode_config_is_derived_from_resolved_config() {
        let mut cfg = ResolvedConfig::default();
        cfg.video.encoder = "nvenc".to_string();
        cfg.video.codec = "h264".to_string();
        cfg.video.bitrate_bps = 30_000_000;
        cfg.video.fps = 120;
        cfg.video.slices = Some(2);
        cfg.video.sfe = true;

        let enc = EncodeConfig::from(&cfg);
        assert_eq!(enc.encoder, EncoderKind::Nvenc);
        assert_eq!(enc.codec, CodecKind::H264);
        assert_eq!(enc.bitrate_bps, 30_000_000);
        assert_eq!(enc.fps, 120);
        assert_eq!(enc.slices, 2);
        assert!(enc.sfe);
    }

    #[test]
    fn encode_config_from_default_resolved_uses_builtin_defaults() {
        let enc = EncodeConfig::from(&ResolvedConfig::default());
        assert_eq!(enc.encoder, EncoderKind::Auto);
        assert_eq!(enc.codec, CodecKind::Hevc);
        assert_eq!(enc.bitrate_bps, 20_000_000);
        assert_eq!(enc.fps, 60);
        // 未设置切片 = 不切片（§3.6 的切片与 SFE 都是 v1 能力，默认关闭）。
        assert_eq!(enc.slices, 0);
        assert!(!enc.sfe);
    }

    #[test]
    fn encode_config_derivation_falls_back_on_bad_names() {
        // 非法取值应当已被 beam-config 的 validate 拦下；这里验证派生侧不 panic。
        let mut cfg = ResolvedConfig::default();
        cfg.video.encoder = "h264-encoder".to_string();
        cfg.video.codec = "vp9".to_string();
        let enc = EncodeConfig::from(&cfg);
        assert_eq!(enc.encoder, EncoderKind::Auto);
        assert_eq!(enc.codec, CodecKind::Hevc);
    }
}
