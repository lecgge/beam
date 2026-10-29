//! 探测本机可用的硬件解码器（§3.10）。
//!
//! 判定依据：
//!
//! 1. ffmpeg 构建里有这个解码器（`avcodec_find_decoder_by_name`）。
//! 2. 该解码器声明了 `AV_PIX_FMT_D3D11` 的硬件配置，
//!    且支持 `AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX`（也就是本实现采用的路线）。
//!
//! **验证状态**：本函数在本机被真实跑通——它读出了本机解码器声明的 D3D11VA 硬件配置；
//! 但这只反映「ffmpeg 构建 + 驱动声明」，不代表真的能解出画面。

use std::sync::OnceLock;

use beam_proto::CodecKind;
use ffmpeg_next::sys as ffi;

use crate::decoder::supports_codec;

/// 解码器能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoderCaps {
    /// 编解码器。
    pub codec: CodecKind,
    /// ffmpeg 里的解码器名。
    pub name: &'static str,
    /// 是否支持 D3D11VA 硬解。
    pub d3d11va: bool,
    /// 最大宽度。
    pub max_width: u32,
    /// 最大高度。
    pub max_height: u32,
    /// 最大帧率。
    pub max_fps: u32,
}

/// 探测结果缓存：进程内只探一次。
static CAPS: OnceLock<Vec<DecoderCaps>> = OnceLock::new();

/// 探测本机可用的解码器。
pub fn probe_decoders() -> Vec<DecoderCaps> {
    CAPS.get_or_init(probe_uncached).clone()
}

/// 不带缓存的探测实现。
fn probe_uncached() -> Vec<DecoderCaps> {
    let _ = ffmpeg_next::init();
    let mut out = Vec::new();
    for codec in [CodecKind::Hevc, CodecKind::H264, CodecKind::Av1] {
        if !supports_codec(codec) {
            continue;
        }
        let name = match crate::ffmpeg::decoder::decoder_name(codec) {
            Some(n) => n,
            None => continue,
        };
        let codec_ptr = crate::ffmpeg::decoder::find_decoder(name);
        if codec_ptr.is_null() {
            continue;
        }
        let d3d11va = supports_d3d11(codec_ptr);
        out.push(DecoderCaps {
            codec,
            name,
            d3d11va,
            // ffmpeg 不暴露解码器的分辨率上限，取各标准常见的保守值。
            max_width: 8192,
            max_height: 4320,
            max_fps: 240,
        });
    }
    out
}

/// 解码器是否声明了 D3D11 硬件配置。
#[allow(unsafe_code, reason = "需要调用 ffmpeg 的硬件配置查询 FFI")]
fn supports_d3d11(codec: *const ffi::AVCodec) -> bool {
    let mut index = 0i32;
    loop {
        let cfg = unsafe { ffi::avcodec_get_hw_config(codec, index) };
        if cfg.is_null() {
            return false;
        }
        let (pix_fmt, methods) = unsafe { ((*cfg).pix_fmt, (*cfg).methods) };
        if pix_fmt == ffi::AVPixelFormat::AV_PIX_FMT_D3D11
            && methods & (ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
        {
            return true;
        }
        index += 1;
    }
}
