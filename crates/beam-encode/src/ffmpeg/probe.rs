//! 真实探测本机可用的硬件编码器（§3.6 `probe()`）。
//!
//! 判定「本机可用」分两步：
//!
//! 1. **ffmpeg 构建里有这个编码器**——`avcodec_find_encoder_by_name` 能找到，
//!    且带 `AV_CODEC_CAP_HARDWARE` 能力位。
//! 2. **本机确实能打开它**——用 §3.6 的参数基线真的 `avcodec_open2` 一次。
//!    驱动缺失、GPU 不存在、参数不被支持都会在这里暴露。
//!
//! 第 2 步打开后**立即释放**，因此探测本身不会长期占用驱动的并发会话额度。
//!
//! **验证状态**：本函数在本机被真实跑通——它确实逐个打开了本机可用的 NVENC 编码器。
//! 在没有可用硬编的机器上，返回空列表是**正确结果**。

use std::sync::OnceLock;

use beam_proto::CodecKind;
use ffmpeg_next::sys as ffi;

use crate::encoder::{EncodeConfig, EncoderCaps, EncoderKind};
use crate::params;

/// 探测结果缓存：进程内只探一次。
static CAPS: OnceLock<Vec<EncoderCaps>> = OnceLock::new();

/// 探测本机可用的硬件编码器（§3.6 `probe()`）。
pub fn probe_encoders() -> Vec<EncoderCaps> {
    CAPS.get_or_init(probe_uncached).clone()
}

/// 不带缓存的探测实现。
fn probe_uncached() -> Vec<EncoderCaps> {
    // 注册 ffmpeg 的编码器表（幂等）。
    let _ = ffmpeg_next::init();
    let mut out = Vec::new();
    for kind in EncoderKind::AUTO_PRIORITY {
        for codec in crate::ffmpeg::encoder::probe_codecs() {
            if let Some(caps) = probe_one(kind, codec) {
                out.push(caps);
            }
        }
    }
    out
}

/// 探测单个（厂商 × 编解码器）组合。
#[allow(
    unsafe_code,
    reason = "需要调用 ffmpeg 的编码器查找/释放 FFI 并读取 AVCodec 能力位"
)]
fn probe_one(kind: EncoderKind, codec: CodecKind) -> Option<EncoderCaps> {
    let name = params::encoder_name(kind, codec)?;
    let codec_ptr = crate::ffmpeg::encoder::find_encoder(name);
    if codec_ptr.is_null() {
        // ffmpeg 构建里没有这个编码器。
        return None;
    }
    // 必须是硬件编码器。
    if unsafe { (*codec_ptr).capabilities } & (ffi::AV_CODEC_CAP_HARDWARE as i32) == 0 {
        return None;
    }

    // 真的打开一次：这是「本机有可用硬编」的唯一可靠判据。
    // 用 NV12 软件输入探测，避免为了探测去建 D3D11 设备上下文。
    let cfg = EncodeConfig {
        encoder: kind,
        codec,
        ..EncodeConfig::default()
    };
    let opened = crate::ffmpeg::encoder::open_encoder(
        codec_ptr,
        kind,
        &cfg,
        ffi::AVPixelFormat::AV_PIX_FMT_NV12,
        None,
        None,
    );
    match opened {
        Ok((ctx, degraded)) => {
            // 立即释放，不占用驱动的并发会话额度。
            let mut ctx = ctx;
            unsafe { ffi::avcodec_free_context(&mut ctx) };
            if !degraded.is_empty() {
                tracing::warn!(
                    encoder = name,
                    ?degraded,
                    "编码器不支持部分 §3.6 参数，已降档"
                );
            }
            Some(caps_for(kind, codec))
        }
        Err(err) => {
            tracing::info!(encoder = name, error = %err, "编码器在本机不可用");
            None
        }
    }
}

/// 能力上限。
///
/// ffmpeg 没有暴露「每个编码器的最大分辨率/帧率」，这里取各厂商公开上限的
/// 保守值（NVENC/AMF 支持 8K 编码；QSV 受 CPU 限制，按 4K 保守取值）。
fn caps_for(kind: EncoderKind, codec: CodecKind) -> EncoderCaps {
    let (max_width, max_height, max_fps) = match kind {
        EncoderKind::Qsv => (4096, 2160, 120),
        _ => (7680, 4320, 240),
    };
    EncoderCaps {
        encoder: kind,
        codec,
        max_width,
        max_height,
        max_fps,
        // 三家的低延迟档（NVENC ull / AMF ultralowlatency / QSV veryfast）都支持。
        low_latency: true,
        // 切片是传输层能力（§6），三家都支持。
        slice: true,
        // SFE（split-frame encoding）目前只有 NVENC 在 Ada 之后支持。
        sfe: kind == EncoderKind::Nvenc,
    }
}
