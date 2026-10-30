//! Opus 编解码（§3.7「Opus 5 ms 帧」）。
//!
//! ## 用 ffmpeg 的 **libopus** 编解码器
//!
//! ffmpeg 里有两个 opus 实现，本模块用 **`libopus`** 那一项：
//!
//! | 实现 | 采样格式 | experimental | 结论 |
//! |---|---|---|---|
//! | `opus`（ffmpeg 内建） | 只支持 `fltp` | **是**（capabilities 带 `exp`） | 不可用：`avcodec_open2` 会直接拒绝 |
//! | `libopus`（外部 libopus 库） | 支持 `s16` / `flt` | 否 | **用它** |
//!
//! §7 要求只分发 **LGPL** 构建的 ffmpeg。libopus 本身是 BSD 许可，
//! LGPL 构建里带 `libopus` 变体是允许的（`--enable-libopus` 不引入 GPL 代码），
//! 本仓库用的 LGPL 构建里也确实带了它。
//!
//! 另一个理由是依赖面：仓库里已经有 `ffmpeg-next` 的用法（`beam-encode`），
//! 用它不引入新的原生依赖。
//!
//! > 之前 `beam-audio` 的 `opus` feature 指的是 `opus` crate（libopus 的 Rust 绑定），
//! > 但那个依赖**从未在 `src/` 里被引用过**，而且它需要单独的 libopus 开发库
//! > （本机与 CI 都没有），所以本模块改用 `ffmpeg-next`。
//!
//! ## 帧长
//!
//! §3.7：48 kHz、5 ms 一帧 ⇒ 单声道每帧 **240 个采样**
//! （[`crate::audio::SAMPLES_PER_FRAME`]）。
//! Opus 原生只支持 120/240/480/960/1920/2880 采样每帧，5 ms 正好对应 240。
//! libopus 编码器的帧长由 `frame_duration`（毫秒）决定，通过
//! `avcodec_open2` 的选项字典传入。
//!
//! ## unsafe 策略（决策 D1）
//!
//! `ffmpeg-next::sys` 是裸 FFI 绑定，必须直接写 `AVCodecContext` / `AVFrame`
//! 的字段并调用 `avcodec_*`。本 crate 用 crate 级 `#![deny(unsafe_code)]`
//! + 局部 `#[allow(unsafe_code)]`（与 `beam-capture` / `beam-encode` 同一口径）。

use bytes::Bytes;

use crate::audio::{AudioConfig, PcmFrame};
use crate::error::AudioError;

/// Opus 编码器核心。
///
/// feature 关闭时是**空实现**（`encode` 返回 `E7004` 后端未启用）。
pub struct EncoderCore {
    inner: Option<EncoderInner>,
    cfg: Option<AudioConfig>,
}

/// Opus 解码器核心。
///
/// feature 关闭时是**空实现**（`decode` 返回 `E7004` 后端未启用）。
pub struct DecoderCore {
    inner: Option<DecoderInner>,
    cfg: Option<AudioConfig>,
}

impl std::fmt::Debug for EncoderCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderCore")
            .field("backend", &self.inner.is_some())
            .field("cfg", &self.cfg)
            .finish()
    }
}

impl std::fmt::Debug for DecoderCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderCore")
            .field("backend", &self.inner.is_some())
            .field("cfg", &self.cfg)
            .finish()
    }
}

#[cfg(feature = "opus")]
use ffmpeg_impl::{DecoderInner, EncoderInner};

/// feature 关闭时的编码器占位（永远不会被构造）。
#[cfg(not(feature = "opus"))]
#[derive(Debug, Default)]
struct EncoderInner;

/// feature 关闭时的解码器占位（永远不会被构造）。
#[cfg(not(feature = "opus"))]
#[derive(Debug, Default)]
struct DecoderInner;

impl EncoderCore {
    /// 新建编码器；feature 关闭时返回 `E7004`。
    pub fn new(cfg: AudioConfig) -> Result<Self, AudioError> {
        #[cfg(feature = "opus")]
        {
            let inner = EncoderInner::open(&cfg)?;
            Ok(Self {
                inner: Some(inner),
                cfg: Some(cfg),
            })
        }
        #[cfg(not(feature = "opus"))]
        {
            let _ = cfg;
            Err(AudioError::BackendDisabled)
        }
    }

    /// 编码一帧 PCM。
    pub fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError> {
        #[cfg(feature = "opus")]
        {
            match self.inner.as_mut() {
                Some(inner) => inner.encode(pcm),
                None => Err(AudioError::BackendDisabled),
            }
        }
        #[cfg(not(feature = "opus"))]
        {
            let _ = pcm;
            Err(AudioError::BackendDisabled)
        }
    }

    /// 重置编码器状态（重连时使用）。
    pub fn reset(&mut self) {
        #[cfg(feature = "opus")]
        if let Some(inner) = self.inner.as_mut() {
            inner.reset();
        }
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&AudioConfig> {
        self.cfg.as_ref()
    }
}

impl DecoderCore {
    /// 新建解码器；feature 关闭时返回 `E7004`。
    pub fn new(cfg: AudioConfig) -> Result<Self, AudioError> {
        #[cfg(feature = "opus")]
        {
            let inner = DecoderInner::open(&cfg)?;
            Ok(Self {
                inner: Some(inner),
                cfg: Some(cfg),
            })
        }
        #[cfg(not(feature = "opus"))]
        {
            let _ = cfg;
            Err(AudioError::BackendDisabled)
        }
    }

    /// 解码一个 Opus 包。
    pub fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError> {
        #[cfg(feature = "opus")]
        {
            match self.inner.as_mut() {
                Some(inner) => inner.decode(packet),
                None => Err(AudioError::BackendDisabled),
            }
        }
        #[cfg(not(feature = "opus"))]
        {
            let _ = packet;
            Err(AudioError::BackendDisabled)
        }
    }

    /// 重置解码器状态。
    pub fn reset(&mut self) {
        #[cfg(feature = "opus")]
        if let Some(inner) = self.inner.as_mut() {
            inner.reset();
        }
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&AudioConfig> {
        self.cfg.as_ref()
    }
}

// ---------------------------------------------------------------------------
// ffmpeg 实现（feature 开启时才有）。
// ---------------------------------------------------------------------------

#[cfg(feature = "opus")]
#[allow(unsafe_code)]
mod ffmpeg_impl {
    use ffmpeg_next::sys as ffi;

    use super::*;
    use crate::audio::{FRAME_DURATION_MS, SAMPLES_PER_FRAME};

    /// 每个采样 2 字节（i16 交错）。
    const BYTES_PER_SAMPLE: usize = 2;

    /// `AVERROR(EAGAIN)`。
    ///
    /// ffmpeg 的 `AVERROR(x)` 是 C 宏，bindgen 不会把它展开成常量，
    /// 所以 Rust 侧要自己算：EAGAIN 在 Windows（MSVC）上是 11，
    /// `AVERROR(e) = -(e)` ⇒ -11。
    const AVERROR_EAGAIN: i32 = -11;

    /// Opus 编码器的原生上下文。
    pub(super) struct EncoderInner {
        ctx: *mut ffi::AVCodecContext,
        frame: *mut ffi::AVFrame,
        packet: *mut ffi::AVPacket,
        /// 送出的采样数累计，用于 pts（1/48000 时基）。
        pts: i64,
        channels: i32,
    }

    // SAFETY: `AVCodecContext` / `AVFrame` / `AVPacket` 都是裸指针，本身不是 `Send`。
    // 这里的契约是：**一个编码器实例只在一个线程里使用**（`OpusEncoder: Send` 的
    // 意义是「可以把编码器整体搬到另一个线程」，而不是「同一个编码器被多线程并发用」）。
    // 编码器不共享任何可变全局状态，跨线程移动是安全的。
    unsafe impl Send for EncoderInner {}

    impl EncoderInner {
        pub(super) fn open(cfg: &AudioConfig) -> Result<Self, AudioError> {
            let codec = unsafe { ffi::avcodec_find_encoder_by_name(c"libopus".as_ptr()) };
            if codec.is_null() {
                return Err(AudioError::CodecFailed {
                    reason: "ffmpeg 里没有 libopus 编码器（LGPL 构建需带 --enable-libopus）"
                        .to_string(),
                });
            }
            let mut ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
            if ctx.is_null() {
                return Err(AudioError::CodecFailed {
                    reason: "分配 opus 编码器上下文失败".to_string(),
                });
            }
            let channels = i32::from(cfg.channels.max(1));
            unsafe {
                (*ctx).sample_rate = cfg.sample_rate as i32;
                (*ctx).channels = channels;
                (*ctx).channel_layout = if channels == 1 {
                    ffi::AV_CH_LAYOUT_MONO
                } else {
                    ffi::AV_CH_LAYOUT_STEREO
                };
                (*ctx).sample_fmt = ffi::AVSampleFormat::AV_SAMPLE_FMT_S16;
                (*ctx).bit_rate = i64::from(cfg.bitrate_bps);
                // Opus 的时基固定 1/48000。
                (*ctx).time_base = ffi::AVRational {
                    num: 1,
                    den: cfg.sample_rate as i32,
                };
                // Opus 只支持固定帧长；5 ms = 240 采样。
                (*ctx).frame_size = SAMPLES_PER_FRAME as i32;
            }
            // libopus 的帧长由 `frame_duration`（毫秒）决定 —— 这是**编码器私有选项**，
            // 只能在 `avcodec_open2` 的选项字典里传（提前用 `av_opt_set_*` 设会返回
            // `AVERROR_OPTION_NOT_FOUND`：私有选项是 open2 期间才注册进上下文的）。
            let mut dict: *mut ffi::AVDictionary = std::ptr::null_mut();
            let ckey = std::ffi::CString::new("frame_duration").expect("字面量不含 NUL");
            let cvalue =
                std::ffi::CString::new(FRAME_DURATION_MS.to_string()).expect("数字不含 NUL");
            unsafe {
                ffi::av_dict_set(&mut dict, ckey.as_ptr(), cvalue.as_ptr(), 0);
            }
            let ret = unsafe { ffi::avcodec_open2(ctx, codec, &mut dict) };
            unsafe {
                ffi::av_dict_free(&mut dict);
            }
            if ret < 0 {
                unsafe { ffi::avcodec_free_context(&mut ctx) };
                return Err(AudioError::CodecFailed {
                    reason: format!("打开 opus 编码器失败（ffmpeg 返回 {ret}）"),
                });
            }
            let mut frame = unsafe { ffi::av_frame_alloc() };
            let mut packet = unsafe { ffi::av_packet_alloc() };
            if frame.is_null() || packet.is_null() {
                unsafe {
                    if !frame.is_null() {
                        ffi::av_frame_free(&mut frame);
                    }
                    if !packet.is_null() {
                        ffi::av_packet_free(&mut packet);
                    }
                    ffi::avcodec_free_context(&mut ctx);
                }
                return Err(AudioError::CodecFailed {
                    reason: "分配 opus 帧/包失败".to_string(),
                });
            }
            Ok(Self {
                ctx,
                frame,
                packet,
                pts: 0,
                channels,
            })
        }

        pub(super) fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError> {
            let samples_per_frame = SAMPLES_PER_FRAME;
            let expected = samples_per_frame * self.channels.max(1) as usize * BYTES_PER_SAMPLE;
            if pcm.samples.len() != expected {
                return Err(AudioError::CodecFailed {
                    reason: format!(
                        "PCM 长度不对：期望 {expected} 字节（{samples_per_frame} 采样 × {} 声道），实际 {}",
                        self.channels,
                        pcm.samples.len()
                    ),
                });
            }
            unsafe {
                // 释放上一帧的数据缓冲，复用同一个 AVFrame。
                ffi::av_frame_unref(self.frame);
                (*self.frame).format = ffi::AVSampleFormat::AV_SAMPLE_FMT_S16 as i32;
                (*self.frame).nb_samples = samples_per_frame as i32;
                (*self.frame).sample_rate = pcm.sample_rate as i32;
                (*self.frame).channels = self.channels;
                (*self.frame).channel_layout = if self.channels == 1 {
                    ffi::AV_CH_LAYOUT_MONO
                } else {
                    ffi::AV_CH_LAYOUT_STEREO
                };
                (*self.frame).pts = self.pts;
                let ret = ffi::av_frame_get_buffer(self.frame, 0);
                if ret < 0 {
                    return Err(AudioError::CodecFailed {
                        reason: format!("分配 opus 编码帧缓冲失败（{ret}）"),
                    });
                }
                // 把 PCM 拷进帧缓冲。
                let dst = std::slice::from_raw_parts_mut((*self.frame).data[0], expected);
                dst.copy_from_slice(&pcm.samples);
                let ret = ffi::avcodec_send_frame(self.ctx, self.frame);
                if ret < 0 {
                    return Err(AudioError::CodecFailed {
                        reason: format!("opus 编码送帧失败（{ret}）"),
                    });
                }
                self.pts += samples_per_frame as i64;

                ffi::av_packet_unref(self.packet);
                let ret = ffi::avcodec_receive_packet(self.ctx, self.packet);
                if ret == AVERROR_EAGAIN {
                    // Opus 编码器有**一帧的流水线延迟**（§3.6 的硬编同理，
                    // 只是那边用 E6005 表达）：头几帧送进去时还取不出包。
                    // 这里返回**空包**，调用方跳过空包即可 ——
                    // 不算错误，也不该在这里阻塞等下一个包（那会把采集节拍拖慢）。
                    return Ok(Bytes::new());
                }
                if ret < 0 {
                    return Err(AudioError::CodecFailed {
                        reason: format!("opus 编码取包失败（{ret}）"),
                    });
                }
                let size = (*self.packet).size.max(0) as usize;
                let data = (*self.packet).data;
                let out = if data.is_null() || size == 0 {
                    Bytes::new()
                } else {
                    Bytes::copy_from_slice(std::slice::from_raw_parts(data, size))
                };
                Ok(out)
            }
        }

        pub(super) fn reset(&mut self) {
            unsafe {
                ffi::avcodec_flush_buffers(self.ctx);
            }
            self.pts = 0;
        }
    }

    impl Drop for EncoderInner {
        fn drop(&mut self) {
            unsafe {
                if !self.frame.is_null() {
                    ffi::av_frame_free(&mut self.frame);
                }
                if !self.packet.is_null() {
                    ffi::av_packet_free(&mut self.packet);
                }
                if !self.ctx.is_null() {
                    ffi::avcodec_free_context(&mut self.ctx);
                }
            }
        }
    }

    /// Opus 解码器的原生上下文。
    pub(super) struct DecoderInner {
        ctx: *mut ffi::AVCodecContext,
        frame: *mut ffi::AVFrame,
        packet: *mut ffi::AVPacket,
    }

    // SAFETY: 同 `EncoderInner` —— 一个解码器实例只在一个线程里使用。
    unsafe impl Send for DecoderInner {}

    impl DecoderInner {
        pub(super) fn open(cfg: &AudioConfig) -> Result<Self, AudioError> {
            let codec = unsafe { ffi::avcodec_find_decoder_by_name(c"libopus".as_ptr()) };
            if codec.is_null() {
                return Err(AudioError::CodecFailed {
                    reason: "ffmpeg 里没有 libopus 解码器（LGPL 构建需带 --enable-libopus）"
                        .to_string(),
                });
            }
            let mut ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
            if ctx.is_null() {
                return Err(AudioError::CodecFailed {
                    reason: "分配 opus 解码器上下文失败".to_string(),
                });
            }
            let channels = i32::from(cfg.channels.max(1));
            unsafe {
                (*ctx).sample_rate = cfg.sample_rate as i32;
                (*ctx).channels = channels;
                (*ctx).channel_layout = if channels == 1 {
                    ffi::AV_CH_LAYOUT_MONO
                } else {
                    ffi::AV_CH_LAYOUT_STEREO
                };
                // 显式指定输出格式：**libopus 解码器默认可能给 flt/fltp（平面浮点）**，
                // 而 `PcmFrame` 存的是**交错 i16** —— 格式对不上会把采样读成垃圾。
                (*ctx).sample_fmt = ffi::AVSampleFormat::AV_SAMPLE_FMT_S16;
            }
            let ret = unsafe { ffi::avcodec_open2(ctx, codec, std::ptr::null_mut()) };
            if ret < 0 {
                unsafe { ffi::avcodec_free_context(&mut ctx) };
                return Err(AudioError::CodecFailed {
                    reason: format!("打开 opus 解码器失败（ffmpeg 返回 {ret}）"),
                });
            }
            let mut frame = unsafe { ffi::av_frame_alloc() };
            let mut packet = unsafe { ffi::av_packet_alloc() };
            if frame.is_null() || packet.is_null() {
                unsafe {
                    if !frame.is_null() {
                        ffi::av_frame_free(&mut frame);
                    }
                    if !packet.is_null() {
                        ffi::av_packet_free(&mut packet);
                    }
                    ffi::avcodec_free_context(&mut ctx);
                }
                return Err(AudioError::CodecFailed {
                    reason: "分配 opus 解码帧/包失败".to_string(),
                });
            }
            Ok(Self { ctx, frame, packet })
        }

        pub(super) fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError> {
            if packet.is_empty() {
                return Err(AudioError::CodecFailed {
                    reason: "空的 opus 包".to_string(),
                });
            }
            unsafe {
                ffi::av_packet_unref(self.packet);
                (*self.packet).data = packet.as_ptr() as *mut u8;
                (*self.packet).size = packet.len() as i32;
                let ret = ffi::avcodec_send_packet(self.ctx, self.packet);
                if ret < 0 {
                    return Err(AudioError::CodecFailed {
                        reason: format!("opus 解码送包失败（{ret}）"),
                    });
                }
                ffi::av_frame_unref(self.frame);
                let ret = ffi::avcodec_receive_frame(self.ctx, self.frame);
                if ret < 0 {
                    return Err(AudioError::CodecFailed {
                        reason: format!("opus 解码取帧失败（{ret}）"),
                    });
                }
                let nb_samples = (*self.frame).nb_samples.max(0) as usize;
                let channels = (*self.frame).channels.max(1) as usize;
                // 校验输出格式确实是交错 i16 —— 格式不符就**报错**，不要把平面浮点
                // 当成 i16 读（那样会得到一帧噪声还不报错）。
                if (*self.frame).format != ffi::AVSampleFormat::AV_SAMPLE_FMT_S16 as i32 {
                    return Err(AudioError::CodecFailed {
                        reason: format!(
                            "opus 解码输出格式不是 S16（实际 {}）",
                            (*self.frame).format
                        ),
                    });
                }
                let bytes = nb_samples * channels * BYTES_PER_SAMPLE;
                let data = (*self.frame).data[0];
                let samples = if data.is_null() || bytes == 0 {
                    Bytes::new()
                } else {
                    Bytes::copy_from_slice(std::slice::from_raw_parts(data, bytes))
                };
                Ok(PcmFrame {
                    samples,
                    sample_rate: (*self.frame).sample_rate as u32,
                    channels: channels as u16,
                })
            }
        }

        pub(super) fn reset(&mut self) {
            unsafe {
                ffi::avcodec_flush_buffers(self.ctx);
            }
        }
    }

    impl Drop for DecoderInner {
        fn drop(&mut self) {
            unsafe {
                if !self.frame.is_null() {
                    ffi::av_frame_free(&mut self.frame);
                }
                if !self.packet.is_null() {
                    ffi::av_packet_free(&mut self.packet);
                }
                if !self.ctx.is_null() {
                    ffi::avcodec_free_context(&mut self.ctx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一帧 1 kHz 正弦（i16 交错），用来验证编解码往返。
    #[cfg(feature = "opus")]
    fn sine_frame(channels: u16, amplitude: i16) -> PcmFrame {
        let n = crate::audio::SAMPLES_PER_FRAME;
        let mut samples = Vec::with_capacity(n * channels as usize * 2);
        for i in 0..n {
            let v = ((i as f32 / 48_000.0) * 2.0 * std::f32::consts::PI * 1_000.0).sin()
                * amplitude as f32;
            for _ in 0..channels {
                samples.extend_from_slice(&(v as i16).to_le_bytes());
            }
        }
        PcmFrame {
            samples: Bytes::from(samples),
            sample_rate: 48_000,
            channels,
        }
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn without_backend_everything_reports_e7004() {
        let enc = EncoderCore::new(AudioConfig::default()).expect_err("必须失败");
        assert_eq!(enc.code(), 7004);
        let dec = DecoderCore::new(AudioConfig::default()).expect_err("必须失败");
        assert_eq!(dec.code(), 7004);
    }

    #[cfg(feature = "opus")]
    #[test]
    fn opus_round_trip_keeps_the_signal() {
        let cfg = AudioConfig {
            mode: crate::audio::AudioMode::HostAndStream,
            channels: 1,
            ..AudioConfig::default()
        };
        let mut enc = EncoderCore::new(cfg.clone()).expect("应当能开编码器");
        let mut dec = DecoderCore::new(cfg).expect("应当能开解码器");

        let pcm = sine_frame(1, 8_000);
        // Opus 有**固有的算法延迟**（5 ms 帧下是受限低延迟模式，约 6.5 ms ≈ 312 采样），
        // 而且**编码器**还有一帧的流水线延迟：
        //   - 编码：头几帧送进去还取不出包（`encode` 返回空包）；
        //   - 解码：第一个包解出来是**延迟填充的静音**（pre-skip）。
        // 所以这里要**连送连解多帧**，再在解码结果里找信号。
        let mut packets: Vec<Bytes> = Vec::new();
        for _ in 0..6 {
            let p = enc.encode(&pcm).expect("编码应当成功");
            if !p.is_empty() {
                packets.push(p);
            }
        }
        assert!(
            packets.len() >= 4,
            "应当至少编出 4 个包，实际 {}",
            packets.len()
        );
        // 5 ms 单声道 128 kbps ≈ 80 字节；给个宽松上界。
        for p in &packets {
            assert!(p.len() < 512, "5 ms 帧不该这么大，实际 {} 字节", p.len());
        }

        let mut decoded: Vec<f64> = Vec::new();
        let mut sample_rate = 0;
        let mut channels = 0;
        for p in &packets {
            let back = dec.decode(p).expect("解码应当成功");
            assert_eq!(back.sample_rate, 48_000);
            assert_eq!(back.channels, 1);
            sample_rate = back.sample_rate;
            channels = back.channels;
            for c in back.samples.chunks_exact(2) {
                decoded.push(i16::from_le_bytes([c[0], c[1]]) as f64);
            }
        }
        assert!(sample_rate == 48_000 && channels == 1);
        assert!(!decoded.is_empty(), "解出来的帧不能是空的");

        // Opus 是有损的，而且**有固有的算法延迟**，所以**不能**逐点比对，
        // 也不能用固定相位去算相关 —— 那样会因为相位差拿到接近 0 的假阴性。
        //
        // 正确的口径是**互相关**：在解码结果里搜索最佳对齐点，
        // 看峰值相关有多高。峰值高说明「信号本身活下来了」。
        let orig: Vec<f64> = pcm
            .samples
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f64)
            .collect();
        let mut best = -1.0f64;
        let mut best_lag = 0usize;
        for lag in 0..=decoded.len().saturating_sub(orig.len()) {
            let mut dot = 0f64;
            let mut e1 = 0f64;
            let mut e2 = 0f64;
            for i in 0..orig.len() {
                let a = orig[i];
                let b = decoded[i + lag];
                dot += a * b;
                e1 += a * a;
                e2 += b * b;
            }
            let corr = dot / (e1 * e2).sqrt().max(1e-9);
            if corr > best {
                best = corr;
                best_lag = lag;
            }
        }
        assert!(
            best > 0.9,
            "编解码往返的最佳互相关应大于 0.9，实际 {best:.3}（滞后 {best_lag} 采样）"
        );
    }

    #[cfg(feature = "opus")]
    #[test]
    fn encoder_rejects_wrong_pcm_length() {
        let cfg = AudioConfig {
            channels: 1,
            ..AudioConfig::default()
        };
        let mut enc = EncoderCore::new(cfg).expect("应当能开编码器");
        let bad = PcmFrame {
            samples: Bytes::from_static(&[0u8; 10]),
            sample_rate: 48_000,
            channels: 1,
        };
        let err = enc.encode(&bad).expect_err("长度不对必须报错");
        assert_eq!(err.code(), 7002);
    }
}
