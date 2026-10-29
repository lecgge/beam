//! 解码器契约与默认实现（§3.10）。
//!
//! §3.10：ffmpeg D3D11VA 硬解 HEVC / H.264（/AV1 v2）；
//! 解码纹理直接交给 D3D11 flip-model 交换链，支持可变刷新率；叠加层由 Tauri 窗口负责。
//!
//! §4 分层说明：§3.10 的签名是 `VideoDecoder::decode(&mut self, f: AssembledFrame)`，
//! 而 `AssembledFrame` 定义在 §3.2（beam-net）。§4 规定 L1 之间只有 net→pair 可以横向依赖，
//! 因此该结构放在 L0 的 beam-proto 里，由 beam-net 重导出。

use beam_proto::{AssembledFrame, CodecKind, DecodedFrame, PlatformHandle};

use crate::error::DecError;

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

/// 默认 D3D11VA 解码器。
///
/// 未启用 `ffmpeg` feature 时不触碰原生库，`decode` 返回 `E10004`。
#[derive(Debug, Default)]
pub struct D3D11VaDecoder {
    cfg: Option<DecodeConfig>,
}

impl D3D11VaDecoder {
    /// 新建解码器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&DecodeConfig> {
        self.cfg.as_ref()
    }

    /// 设置配置（启动解码器前调用）。
    pub fn configure(&mut self, cfg: DecodeConfig) {
        self.cfg = Some(cfg);
    }
}

impl VideoDecoder for D3D11VaDecoder {
    fn decode(&mut self, f: AssembledFrame) -> Result<DecodedFrame, DecError> {
        #[cfg(feature = "ffmpeg")]
        {
            let _ = f;
            // 真实实现：D3D11VA 硬件解码 → 输出 D3D11 纹理。
            Err(DecError::BackendDisabled)
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
            // 真实实现：销毁并重建 D3D11VA 设备与解码上下文。
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
        assert_eq!(err.code(), 10004);
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
