//! PCM 采样格式、协商、转换与分帧（**纯逻辑**，平台无关）。
//!
//! ## 为什么要有这一层
//!
//! §3.7 的 PCM 契约是 **i16 交错**（[`crate::audio::PcmFrame`]），
//! 但 WASAPI 设备的混音格式（mix format）**不一定**是 i16：
//!
//! - 很多设备的混音格式是 **f32**（`WAVE_FORMAT_IEEE_FLOAT`，或
//!   `WAVE_FORMAT_EXTENSIBLE` + `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`）；
//! - 采样率可能是 44.1 kHz，而 §3.7 要 48 kHz（Opus 也只吃 48 kHz）；
//! - 声道数可能是 6（5.1），而配置是 2。
//!
//! 这些都**没法靠单测验证**（要真机），所以把「解析 / 协商 / 转换 / 分帧」
//! 全拆成**纯函数**放在这一层：单测覆盖它们，
//! 真机层（[`crate::wasapi`]）只负责 COM 调用与搬字节。
//!
//! ## 平面 vs 交错
//!
//! WASAPI 的 `WAVEFORMATEX` **恒是交错**的；「平面」（`data[0]`=左声道、
//! `data[1]`=右声道）是 ffmpeg 的 `fltp`/`s16p` 布局。
//! 本模块两种都支持，是因为**读错布局会播噪声**：
//! 把平面数据当交错读，声道会左右错位、时间轴也错位。
//! 真机路径只会用到交错，但转换函数按布局参数走，
//! 以后接 ffmpeg 源（`AV_SAMPLE_FMT_FLTP`）时不用再改一遍。

use bytes::Bytes;

use crate::audio::{PcmFrame, FRAME_DURATION_MS};

/// WAVEFORMATEX 的 `wFormatTag`：整数 PCM。
pub const WAVE_FORMAT_PCM: u16 = 0x0001;

/// WAVEFORMATEX 的 `wFormatTag`：IEEE 浮点。
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;

/// WAVEFORMATEX 的 `wFormatTag`：EXTENSIBLE（真正的格式藏在 `SubFormat` 里）。
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// `KSDATAFORMAT_SUBTYPE_PCM`（EXTENSIBLE 的整数 PCM 子格式 GUID）。
pub const KSDATAFORMAT_SUBTYPE_PCM: u128 = 0x0000_0001_0000_0010_8000_00aa_0038_9b71;

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`（EXTENSIBLE 的浮点子格式 GUID）。
pub const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: u128 = 0x0000_0003_0000_0010_8000_00aa_0038_9b71;

/// i16 → f32 的缩放因子（`-32768..=32767` → `-1.0..=1.0`）。
const I16_TO_F32: f32 = 1.0 / 32_768.0;

/// f32 → i16 的缩放因子。
const F32_TO_I16: f32 = 32_768.0;

/// 采样值的存储类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// 16 位有符号整数（本 crate 的 PCM 契约）。
    I16,
    /// 32 位 IEEE 浮点（WASAPI 设备混音格式常见）。
    F32,
}

/// 声道在内存里的排布。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleLayout {
    /// 交错：`L R L R …`（WASAPI 恒是这个）。
    Interleaved,
    /// 平面：`LLLL… RRRR…`（ffmpeg 的 `fltp`/`s16p`；`data[0]`=左声道、`data[1]`=右声道）。
    Planar,
}

/// WAVEFORMATEX / WAVEFORMATEXTENSIBLE 的原始字段（平台无关的镜像）。
///
/// 真机层从 COM 指针里把字段**拷贝**出来（`WAVEFORMATEX` 是 packed 结构，
/// 不能取引用），再交给 [`MixFormat::from_raw`] 解析。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawWaveFormat {
    /// `wFormatTag`。
    pub format_tag: u16,
    /// `nChannels`。
    pub channels: u16,
    /// `nSamplesPerSec`。
    pub sample_rate: u32,
    /// `nAvgBytesPerSec`。
    pub avg_bytes_per_sec: u32,
    /// `nBlockAlign`。
    pub block_align: u16,
    /// `wBitsPerSample`。
    pub bits_per_sample: u16,
    /// `cbSize`（EXTENSIBLE 时为扩展部分长度，典型 22）。
    pub cb_size: u16,
    /// EXTENSIBLE 的 `SubFormat` GUID（`None` = 非 EXTENSIBLE）。
    pub sub_format: Option<u128>,
}

/// 设备混音格式（解析后的纯数据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixFormat {
    /// 采样值类型。
    pub sample_format: SampleFormat,
    /// 声道排布（WASAPI 恒为交错）。
    pub layout: SampleLayout,
    /// 采样率。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 每采样位数。
    pub bits_per_sample: u16,
}

impl MixFormat {
    /// 解析 WAVEFORMATEX 字段。
    ///
    /// 只支持 **16 位整数 PCM** 与 **32 位浮点**两种；其它（ADPCM、Dolby、
    /// 24 位整数……）返回 `Err`，原因写给日志 —— 宁可明确报错，
    /// 也不要「按 i16 读」读出一段噪声。
    pub fn from_raw(raw: &RawWaveFormat) -> Result<Self, String> {
        if raw.channels == 0 {
            return Err("设备混音格式的声道数为 0".to_string());
        }
        if raw.sample_rate == 0 {
            return Err("设备混音格式的采样率为 0".to_string());
        }
        let (sample_format, bits) = match raw.format_tag {
            WAVE_FORMAT_PCM => (SampleFormat::I16, raw.bits_per_sample),
            WAVE_FORMAT_IEEE_FLOAT => (SampleFormat::F32, raw.bits_per_sample),
            WAVE_FORMAT_EXTENSIBLE => match raw.sub_format {
                Some(KSDATAFORMAT_SUBTYPE_PCM) => (SampleFormat::I16, raw.bits_per_sample),
                Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) => (SampleFormat::F32, raw.bits_per_sample),
                Some(other) => {
                    return Err(format!(
                        "设备混音格式是 EXTENSIBLE，但 SubFormat 不支持（{other:#034x}）"
                    ));
                }
                None => return Err("设备混音格式是 EXTENSIBLE，却读不到 SubFormat".to_string()),
            },
            other => {
                return Err(format!(
                    "设备混音格式不支持（wFormatTag={other:#06x}，只支持整数 PCM 与 IEEE 浮点）"
                ));
            }
        };
        match (sample_format, bits) {
            (SampleFormat::I16, 16) => {}
            (SampleFormat::F32, 32) => {}
            (_, bits) => {
                return Err(format!("设备混音格式的采样位宽不支持（{bits} 位）"));
            }
        }
        Ok(Self {
            sample_format,
            // WASAPI 的 WAVEFORMATEX 恒是交错；平面布局只可能来自别的来源。
            layout: SampleLayout::Interleaved,
            sample_rate: raw.sample_rate,
            channels: raw.channels,
            bits_per_sample: raw.bits_per_sample,
        })
    }

    /// 每帧（每声道一个采样）占用的字节数。
    pub fn frame_bytes(&self) -> usize {
        let bytes = match self.sample_format {
            SampleFormat::I16 => 2,
            SampleFormat::F32 => 4,
        };
        bytes * usize::from(self.channels.max(1))
    }
}

/// 我们**想要**的 PCM 形态：i16 交错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesiredPcm {
    /// 采样率。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
}

/// 播放方案（决定「用设备混音格式」还是「直接写 i16」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackPlan {
    /// 设备混音格式**就是**目标 i16 交错格式：直接写，不转换。
    Native,
    /// 设备混音格式是整数 PCM，且引擎认我们想要的 i16 格式：
    /// 用 `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` 让**引擎**转，我们直接写 i16。
    AutoConvert,
    /// 用设备混音格式，自己转（i16 交错 → 设备格式）。
    Convert(MixFormat),
}

/// 决定播放方案。
///
/// - `native_supported`：`IAudioClient::IsFormatSupported` 对目标 i16 格式
///   是否返回 **S_OK**（严格是 0；S_FALSE 不算）。
/// - 混音格式已经是目标 i16 格式时，一律走 [`PlaybackPlan::Native`]。
/// - 只有「混音格式是整数 PCM」才可能走 [`PlaybackPlan::AutoConvert`]：
///   `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` 只在混音格式是整数 PCM 时有效。
pub fn plan_playback(
    native_supported: bool,
    mix: &MixFormat,
    desired: &DesiredPcm,
) -> PlaybackPlan {
    let mix_is_desired = mix.sample_format == SampleFormat::I16
        && mix.sample_rate == desired.sample_rate
        && mix.channels == desired.channels;
    if mix_is_desired {
        return PlaybackPlan::Native;
    }
    if native_supported && mix.sample_format == SampleFormat::I16 {
        return PlaybackPlan::AutoConvert;
    }
    PlaybackPlan::Convert(*mix)
}

/// 把字节读成 i16（小端）。
pub fn bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// 把 i16 写成字节（小端）。
pub fn i16_to_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// 把字节读成 f32（小端）。
pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 把 f32 写成字节（小端）。
pub fn f32_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// 把交错的 i16 重排成平面（每声道一段）。
fn interleaved_to_planar(samples: &[i16], channels: u16) -> Vec<i16> {
    let ch = usize::from(channels.max(1));
    let frames = samples.len() / ch;
    let mut out = vec![0i16; samples.len()];
    for c in 0..ch {
        for f in 0..frames {
            out[c * frames + f] = samples[f * ch + c];
        }
    }
    out
}

/// 把平面的 i16 重排成交错。
fn planar_to_interleaved(samples: &[i16], channels: u16) -> Vec<i16> {
    let ch = usize::from(channels.max(1));
    let frames = samples.len() / ch;
    let mut out = vec![0i16; frames * ch];
    for c in 0..ch {
        for f in 0..frames {
            out[f * ch + c] = samples[c * frames + f];
        }
    }
    out
}

/// 声道重映射（交错 i16 进，交错 i16 出）。
///
/// - 目标声道更少：取前 N 个声道（WASAPI 混音格式的声道顺序是 L,R,…）。
/// - 目标声道更多：多出来的声道补**静音** —— 「复制 L 到 R」属于混音策略，
///   超出本 crate「不做混音 / 回声消除」的边界（§3.7）。
pub fn remap_channels(src: &[i16], src_channels: u16, dst_channels: u16) -> Vec<i16> {
    let src_ch = usize::from(src_channels.max(1));
    let dst_ch = usize::from(dst_channels.max(1));
    if src_ch == dst_ch {
        return src.to_vec();
    }
    let frames = src.len() / src_ch;
    let mut out = vec![0i16; frames * dst_ch];
    for f in 0..frames {
        for c in 0..dst_ch.min(src_ch) {
            out[f * dst_ch + c] = src[f * src_ch + c];
        }
    }
    out
}

/// 线性重采样（交错 i16）。
///
/// 够用即可：这里只解决「设备 44.1 kHz ↔ 我们 48 kHz」这档差异，
/// 不是高质量 SRC（那会引入带外滤波与状态，属于另一层的事）。
pub fn resample_linear(src: &[i16], channels: u16, from_rate: u32, to_rate: u32) -> Vec<i16> {
    let ch = usize::from(channels.max(1));
    if from_rate == to_rate || from_rate == 0 || to_rate == 0 {
        return src.to_vec();
    }
    let frames = src.len() / ch;
    if frames == 0 {
        return Vec::new();
    }
    let out_frames = ((u64::try_from(frames).unwrap_or(u64::MAX) * u64::from(to_rate))
        / u64::from(from_rate))
    .max(1) as usize;
    let mut out = Vec::with_capacity(out_frames * ch);
    for i in 0..out_frames {
        let pos = i as f64 * f64::from(from_rate) / f64::from(to_rate);
        let i0 = (pos.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let frac = pos - i0 as f64;
        for c in 0..ch {
            let a = f64::from(src[i0 * ch + c]);
            let b = f64::from(src[i1 * ch + c]);
            let v = (a + (b - a) * frac).round();
            out.push(v.clamp(-32_768.0, 32_767.0) as i16);
        }
    }
    out
}

/// 把交错 i16 PCM 转成设备格式的字节。
///
/// 顺序固定：**先重映射声道 → 再重采样 → 最后换采样格式**，
/// 这样重采样是在目标声道数上做的，不会把补出来的静音一起插值。
pub fn pcm_to_device(src: &[i16], src_channels: u16, src_rate: u32, target: &MixFormat) -> Vec<u8> {
    let mut samples = remap_channels(src, src_channels, target.channels);
    if src_rate != target.sample_rate {
        samples = resample_linear(&samples, target.channels, src_rate, target.sample_rate);
    }
    match target.sample_format {
        SampleFormat::I16 => {
            if target.layout == SampleLayout::Planar {
                samples = interleaved_to_planar(&samples, target.channels);
            }
            i16_to_bytes(&samples)
        }
        SampleFormat::F32 => {
            let values: Vec<f32> = samples.iter().map(|v| *v as f32 * I16_TO_F32).collect();
            match target.layout {
                SampleLayout::Interleaved => f32_to_bytes(&values),
                SampleLayout::Planar => {
                    let planar = interleaved_to_planar_f32(&values, target.channels);
                    f32_to_bytes(&planar)
                }
            }
        }
    }
}

/// 把浮点交错采样重排成平面。
fn interleaved_to_planar_f32(samples: &[f32], channels: u16) -> Vec<f32> {
    let ch = usize::from(channels.max(1));
    let frames = samples.len() / ch;
    let mut out = vec![0.0f32; samples.len()];
    for c in 0..ch {
        for f in 0..frames {
            out[c * frames + f] = samples[f * ch + c];
        }
    }
    out
}

/// 把设备格式的字节转成 i16 交错 PCM。
///
/// 与 [`pcm_to_device`] 是逆运算：顺序是**先换格式 → 再重映射声道 → 再重采样**。
pub fn device_to_pcm(
    bytes: &[u8],
    mix: &MixFormat,
    target_rate: u32,
    target_channels: u16,
) -> Vec<i16> {
    let mut samples = match mix.sample_format {
        SampleFormat::I16 => bytes_to_i16(bytes),
        SampleFormat::F32 => bytes_to_f32(bytes)
            .into_iter()
            .map(|v| {
                let scaled = v * F32_TO_I16;
                scaled.clamp(-32_768.0, 32_767.0).round() as i16
            })
            .collect(),
    };
    if mix.layout == SampleLayout::Planar {
        samples = planar_to_interleaved(&samples, mix.channels);
    }
    let mut samples = remap_channels(&samples, mix.channels, target_channels);
    if mix.sample_rate != target_rate {
        samples = resample_linear(&samples, target_channels, mix.sample_rate, target_rate);
    }
    samples
}

/// 给定采样率下，§3.7 的 5 ms 一帧有多少个采样。
pub fn samples_per_frame(sample_rate: u32) -> usize {
    (sample_rate as usize * FRAME_DURATION_MS as usize) / 1000
}

/// 把变长的设备缓冲累积成固定长度的帧（§3.7 的 5 ms 帧）。
///
/// WASAPI 设备的缓冲是**变长**的（10 ms / 20 ms 常见），
/// 而 §3.7 要求吐给编码器的是**固定 5 ms**（240 采样 @ 48 kHz）的帧，
/// 所以这里做累积：凑齐一帧才吐，剩下的留在 `pending` 里等下一块。
#[derive(Debug, Clone)]
pub struct FrameAssembler {
    channels: u16,
    sample_rate: u32,
    samples_per_frame: usize,
    pending: Vec<i16>,
}

impl FrameAssembler {
    /// 新建累积器。
    pub fn new(channels: u16, sample_rate: u32, samples_per_frame: usize) -> Self {
        Self {
            channels: channels.max(1),
            sample_rate,
            samples_per_frame: samples_per_frame.max(1),
            pending: Vec::new(),
        }
    }

    /// 按 §3.7 的 5 ms 帧长新建（48 kHz ⇒ 240 采样）。
    pub fn with_default_frame(channels: u16, sample_rate: u32) -> Self {
        Self::new(channels, sample_rate, samples_per_frame(sample_rate))
    }

    /// 喂入交错 i16 采样，返回凑齐的帧。
    pub fn push(&mut self, samples: &[i16]) -> Vec<PcmFrame> {
        let ch = usize::from(self.channels);
        self.pending.extend_from_slice(samples);
        let need = self.samples_per_frame * ch;
        let mut frames = Vec::new();
        while self.pending.len() >= need {
            let take: Vec<i16> = self.pending.drain(..need).collect();
            frames.push(PcmFrame {
                samples: Bytes::from(i16_to_bytes(&take)),
                sample_rate: self.sample_rate,
                channels: self.channels,
            });
        }
        frames
    }

    /// 还没凑齐一帧的采样数（声道数已乘进去）。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 丢掉累积的残留（重连 / 停机时用）。
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_pcm(rate: u32, channels: u16) -> RawWaveFormat {
        RawWaveFormat {
            format_tag: WAVE_FORMAT_PCM,
            channels,
            sample_rate: rate,
            avg_bytes_per_sec: rate * u32::from(channels) * 2,
            block_align: channels * 2,
            bits_per_sample: 16,
            cb_size: 0,
            sub_format: None,
        }
    }

    #[test]
    fn parses_pcm_mix_format() {
        let mix = MixFormat::from_raw(&raw_pcm(48_000, 2)).expect("应当解析成功");
        assert_eq!(mix.sample_format, SampleFormat::I16);
        assert_eq!(mix.layout, SampleLayout::Interleaved);
        assert_eq!(mix.sample_rate, 48_000);
        assert_eq!(mix.channels, 2);
        assert_eq!(mix.frame_bytes(), 4);
    }

    #[test]
    fn parses_ieee_float_mix_format() {
        let mut raw = raw_pcm(48_000, 2);
        raw.format_tag = WAVE_FORMAT_IEEE_FLOAT;
        raw.bits_per_sample = 32;
        raw.block_align = 8;
        let mix = MixFormat::from_raw(&raw).expect("应当解析成功");
        assert_eq!(mix.sample_format, SampleFormat::F32);
        assert_eq!(mix.frame_bytes(), 8);
    }

    #[test]
    fn parses_extensible_float_mix_format() {
        let mut raw = raw_pcm(48_000, 2);
        raw.format_tag = WAVE_FORMAT_EXTENSIBLE;
        raw.bits_per_sample = 32;
        raw.block_align = 8;
        raw.cb_size = 22;
        raw.sub_format = Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        let mix = MixFormat::from_raw(&raw).expect("应当解析成功");
        assert_eq!(mix.sample_format, SampleFormat::F32);
    }

    #[test]
    fn rejects_unsupported_mix_formats() {
        // ADPCM：既不是 PCM 也不是浮点。
        let mut raw = raw_pcm(48_000, 2);
        raw.format_tag = 0x0011;
        assert!(MixFormat::from_raw(&raw).is_err());

        // 24 位整数 PCM：本 crate 只处理 16 位。
        let mut raw = raw_pcm(48_000, 2);
        raw.bits_per_sample = 24;
        assert!(MixFormat::from_raw(&raw).is_err());

        // 声道数为 0：非法。
        let mut raw = raw_pcm(48_000, 0);
        raw.channels = 0;
        assert!(MixFormat::from_raw(&raw).is_err());
    }

    #[test]
    fn plans_native_when_mix_format_already_matches() {
        let mix = MixFormat::from_raw(&raw_pcm(48_000, 2)).expect("解析成功");
        let desired = DesiredPcm {
            sample_rate: 48_000,
            channels: 2,
        };
        // 即使 IsFormatSupported 说支持，混音格式已经是目标形态 ⇒ Native。
        assert_eq!(plan_playback(true, &mix, &desired), PlaybackPlan::Native);
        assert_eq!(plan_playback(false, &mix, &desired), PlaybackPlan::Native);
    }

    #[test]
    fn plans_convert_when_device_is_float_or_other_rate() {
        let mut raw = raw_pcm(48_000, 2);
        raw.format_tag = WAVE_FORMAT_IEEE_FLOAT;
        raw.bits_per_sample = 32;
        let float_mix = MixFormat::from_raw(&raw).expect("解析成功");
        let desired = DesiredPcm {
            sample_rate: 48_000,
            channels: 2,
        };
        // 混音格式是 f32：即便引擎说支持 i16，也只能自己转（AUTOCONVERTPCM 不适用）。
        assert_eq!(
            plan_playback(true, &float_mix, &desired),
            PlaybackPlan::Convert(float_mix)
        );

        // 44.1 kHz 的整数 PCM：可以交给引擎转。
        let pcm_441 = MixFormat::from_raw(&raw_pcm(44_100, 2)).expect("解析成功");
        assert_eq!(
            plan_playback(true, &pcm_441, &desired),
            PlaybackPlan::AutoConvert
        );
        // 引擎不支持 ⇒ 自己转。
        assert_eq!(
            plan_playback(false, &pcm_441, &desired),
            PlaybackPlan::Convert(pcm_441)
        );
    }

    #[test]
    fn converts_i16_to_float_interleaved() {
        let target = MixFormat {
            sample_format: SampleFormat::F32,
            layout: SampleLayout::Interleaved,
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
        };
        let src = vec![0i16, 16_384, -16_384, 32_767];
        let out = pcm_to_device(&src, 2, 48_000, &target);
        let values = bytes_to_f32(&out);
        assert_eq!(values.len(), 4);
        assert!((values[0] - 0.0).abs() < 1e-6);
        assert!((values[1] - 0.5).abs() < 1e-6, "实际 {}", values[1]);
        assert!((values[2] + 0.5).abs() < 1e-6, "实际 {}", values[2]);
        assert!(values[3] < 1.0 && values[3] > 0.99, "实际 {}", values[3]);
    }

    #[test]
    fn converts_i16_to_float_planar_with_channel_planes() {
        let target = MixFormat {
            sample_format: SampleFormat::F32,
            layout: SampleLayout::Planar,
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
        };
        // 两帧：L0 R0 L1 R1
        let src = vec![100i16, -100, 200, -200];
        let out = pcm_to_device(&src, 2, 48_000, &target);
        let values = bytes_to_f32(&out);
        assert_eq!(values.len(), 4);
        // 平面：data[0] 是左声道（L0, L1），data[1] 是右声道（R0, R1）。
        assert!((values[0] - 100.0 / 32_768.0).abs() < 1e-6);
        assert!((values[1] - 200.0 / 32_768.0).abs() < 1e-6);
        assert!((values[2] + 100.0 / 32_768.0).abs() < 1e-6);
        assert!((values[3] + 200.0 / 32_768.0).abs() < 1e-6);
    }

    #[test]
    fn converts_device_float_back_to_i16() {
        let mix = MixFormat {
            sample_format: SampleFormat::F32,
            layout: SampleLayout::Interleaved,
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
        };
        let device = f32_to_bytes(&[0.5f32, -0.5, 1.0, -1.0]);
        let back = device_to_pcm(&device, &mix, 48_000, 2);
        assert_eq!(back, vec![16_384, -16_384, 32_767, -32_768]);
    }

    #[test]
    fn converts_device_planar_float_back_to_interleaved_i16() {
        let mix = MixFormat {
            sample_format: SampleFormat::F32,
            layout: SampleLayout::Planar,
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
        };
        // 平面：[L0, L1, R0, R1]
        let device = f32_to_bytes(&[0.5f32, 0.25, -0.5, -0.25]);
        let back = device_to_pcm(&device, &mix, 48_000, 2);
        assert_eq!(back, vec![16_384, -16_384, 8_192, -8_192]);
    }

    #[test]
    fn remaps_channels_and_pads_with_silence() {
        // 立体声 → 单声道：取左声道。
        let src = vec![10i16, 20, 30, 40];
        assert_eq!(remap_channels(&src, 2, 1), vec![10, 30]);
        // 单声道 → 立体声：多出来的声道补静音。
        let src = vec![10i16, 30];
        assert_eq!(remap_channels(&src, 1, 2), vec![10, 0, 30, 0]);
    }

    #[test]
    fn resample_keeps_the_signal_frequency() {
        // 1 kHz 正弦，48 kHz → 44.1 kHz：过零点数应当仍然是 1 s 里 1000 个。
        let rate_in = 48_000u32;
        let rate_out = 44_100u32;
        let n = rate_in as usize;
        let src: Vec<i16> = (0..n)
            .map(|i| {
                let v =
                    ((i as f64 / f64::from(rate_in)) * 2.0 * std::f64::consts::PI * 1000.0).sin();
                (v * 8000.0) as i16
            })
            .collect();
        let out = resample_linear(&src, 1, rate_in, rate_out);
        let expected = (n as u64 * u64::from(rate_out) / u64::from(rate_in)) as usize;
        assert!(
            out.len().abs_diff(expected) <= 1,
            "输出长度 {} 与期望 {} 应当接近",
            out.len(),
            expected
        );
        let crossings = out.windows(2).filter(|w| w[0] <= 0 && w[1] > 0).count();
        assert!(
            (980..=1020).contains(&crossings),
            "过零点数应当接近 1000，实际 {crossings}"
        );
    }

    #[test]
    fn resample_is_identity_for_same_rate() {
        let src = vec![1i16, -2, 3];
        assert_eq!(resample_linear(&src, 1, 48_000, 48_000), src);
    }

    #[test]
    fn assembler_emits_fixed_5ms_frames() {
        let mut asm = FrameAssembler::with_default_frame(2, 48_000);
        // 设备给了 20 ms（960 帧 × 2 声道 = 1920 个交错采样）
        // ⇒ 应当切成 4 帧 5 ms。
        let samples = vec![7i16; 960 * 2];
        let frames = asm.push(&samples);
        assert_eq!(frames.len(), 4);
        for f in &frames {
            assert_eq!(f.samples.len(), 240 * 2 * 2);
            assert_eq!(f.sample_rate, 48_000);
            assert_eq!(f.channels, 2);
        }
        assert_eq!(asm.pending_len(), 0);
    }

    #[test]
    fn assembler_keeps_the_remainder() {
        let mut asm = FrameAssembler::with_default_frame(1, 48_000);
        // 10 ms（480 采样）⇒ 2 帧 5 ms。
        let frames = asm.push(&vec![1i16; 480]);
        assert_eq!(frames.len(), 2);
        // 再来 100 采样：不够一帧，先攒着。
        let frames = asm.push(&[2i16; 100]);
        assert!(frames.is_empty());
        assert_eq!(asm.pending_len(), 100);
        // 再来 140 采样 ⇒ 凑齐一帧。
        let frames = asm.push(&vec![3i16; 140]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].samples.len(), 240 * 2);
        assert_eq!(asm.pending_len(), 0);
    }

    #[test]
    fn samples_per_frame_follows_the_frame_duration() {
        assert_eq!(samples_per_frame(48_000), 240);
        assert_eq!(samples_per_frame(44_100), 220);
    }
}
