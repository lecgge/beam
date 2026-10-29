//! §3.6 参数基线（NVENC / AMF / QSV 的低延迟档）与传输层切片。
//!
//! §3.6 第 131 行把参数基线写死为：
//!
//! > NVENC P1 + ultra-low-latency、CBR、无 B 帧、无 lookahead、VBV = 1 帧、intra-refresh；
//! > Ada 上 T1 及高帧率档开启 SFE；AMF `ultralowlatency` + `vbr_latency`；QSV `veryfast`。
//!
//! 本模块把这些条目翻译成
//!
//! - **编码器私有选项**（`AVDictionary`，随 `avcodec_open2` 下发）：
//!   见 [`baseline_options`]；
//! - **编码器上下文字段**（`AVCodecContext`）：见 [`context_settings`]；
//! - **传输层切片**（`EncodedFrame.slices`）：见 [`split_into_slices`]。
//!
//! 本模块**不依赖 ffmpeg**，因此在默认构建下也能编译并被单元测试覆盖。

use beam_proto::CodecKind;

use crate::encoder::{EncodeConfig, EncoderKind};

/// §3.6：编码器在 ffmpeg 里的名字（厂商 × 编解码器）。
///
/// 返回 `None` 表示该组合没有对应的 ffmpeg 编码器
/// （例如 `av1` feature 关闭时的 AV1，见 [`crate::encoder::supports_codec`]）。
pub fn encoder_name(kind: EncoderKind, codec: CodecKind) -> Option<&'static str> {
    match (kind, codec) {
        (EncoderKind::Nvenc, CodecKind::H264) => Some("h264_nvenc"),
        (EncoderKind::Nvenc, CodecKind::Hevc) => Some("hevc_nvenc"),
        (EncoderKind::Nvenc, CodecKind::Av1) => Some("av1_nvenc"),
        (EncoderKind::Amf, CodecKind::H264) => Some("h264_amf"),
        (EncoderKind::Amf, CodecKind::Hevc) => Some("hevc_amf"),
        (EncoderKind::Amf, CodecKind::Av1) => Some("av1_amf"),
        (EncoderKind::Qsv, CodecKind::H264) => Some("h264_qsv"),
        (EncoderKind::Qsv, CodecKind::Hevc) => Some("hevc_qsv"),
        (EncoderKind::Qsv, CodecKind::Av1) => Some("av1_qsv"),
        (EncoderKind::Auto, _) => None,
    }
}

/// §3.6：把编码配置翻译成 ffmpeg 编码器私有选项（`AVDictionary`）。
///
/// 逐条对应 §3.6 的参数基线：
///
/// | 厂商 | 选项 | 对应基线 |
/// |---|---|---|
/// | NVENC | `preset=p1` | P1 |
/// | NVENC | `tune=ull` | ultra-low-latency |
/// | NVENC | `rc=cbr` | CBR |
/// | NVENC | `rc-lookahead=0` | 无 lookahead |
/// | NVENC | `intra-refresh=1` | intra-refresh |
/// | NVENC | `forced-idr=1` | 强制 IDR（§3.6「强制 IDR」） |
/// | NVENC | `split_mode=rowcol` | Ada SFE（仅 `sfe=true` 时下发） |
/// | AMF | `usage=ultralowlatency` | ultralowlatency |
/// | AMF | `rc=vbr_latency` | vbr_latency |
/// | QSV | `preset=veryfast` | veryfast |
/// | QSV | `look_ahead=0` / `bf=0` / `async_depth=1` | 无 lookahead、无 B 帧、低延迟 |
///
/// 「无 B 帧」对三家都额外通过 `AVCodecContext.max_b_frames = 0` 落实，
/// 见 [`context_settings`]。
pub fn baseline_options(kind: EncoderKind, cfg: &EncodeConfig) -> Vec<(&'static str, String)> {
    let mut opts: Vec<(&'static str, String)> = Vec::new();
    match kind {
        EncoderKind::Nvenc => {
            opts.push(("preset", "p1".to_string()));
            opts.push(("tune", "ull".to_string()));
            opts.push(("rc", "cbr".to_string()));
            opts.push(("rc-lookahead", "0".to_string()));
            opts.push(("intra-refresh", "1".to_string()));
            opts.push(("forced-idr", "1".to_string()));
            if cfg.sfe {
                // Ada（RTX 40 系）起 NVENC 支持 split-frame encoding。
                opts.push(("split_mode", "rowcol".to_string()));
            }
        }
        EncoderKind::Amf => {
            opts.push(("usage", "ultralowlatency".to_string()));
            opts.push(("rc", "vbr_latency".to_string()));
        }
        EncoderKind::Qsv => {
            opts.push(("preset", "veryfast".to_string()));
            opts.push(("look_ahead", "0".to_string()));
            opts.push(("bf", "0".to_string()));
            opts.push(("async_depth", "1".to_string()));
        }
        EncoderKind::Auto => {}
    }
    opts
}

/// 编码器不支持某个选项时的**降档顺序**（越靠前越先被丢弃）。
///
/// §3.6 的 `E6003` 语义是「参数不支持 → 降档」：打开编码器时若 ffmpeg 报
/// `AVERROR_OPTION_NOT_FOUND`，就按这个顺序丢掉可选项后重试，
/// 并把被丢弃的项回报给调用方。
///
/// `preset` / `usage` / `rc` 是基线的核心，**不参与降档**——丢了它们就不是
/// §3.6 描述的那一档了，应当直接失败。
pub const DROPPABLE_OPTIONS: [&str; 8] = [
    "split_mode",
    "intra-refresh",
    "forced-idr",
    "rc-lookahead",
    "async_depth",
    "bf",
    "look_ahead",
    "tune",
];

/// §3.6：`AVCodecContext` 上的低延迟设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSettings {
    /// 目标码率，bit/s（`bit_rate`）。
    pub bit_rate: i64,
    /// 码率上限，bit/s（`rc_max_rate`）；CBR 下与 `bit_rate` 相同。
    pub rc_max_rate: i64,
    /// VBV 缓冲大小，bit（`rc_buffer_size`）；§3.6 要求「VBV = 1 帧」。
    pub rc_buffer_size: i32,
    /// B 帧数上限（`max_b_frames`）；§3.6 要求「无 B 帧」，恒为 0。
    pub max_b_frames: i32,
    /// GOP 长度（`gop_size`），帧。
    pub gop_size: i32,
    /// 时基（`time_base`），分子/分母。
    pub time_base: (i32, i32),
    /// 帧率（`framerate`），分子/分母。
    pub framerate: (i32, i32),
    /// 是否置 `AV_CODEC_FLAG_LOW_DELAY`。
    pub low_delay: bool,
}

/// §3.6：把编码配置翻译成 `AVCodecContext` 字段。
///
/// - 「无 B 帧」→ `max_b_frames = 0`。
/// - 「VBV = 1 帧」→ `rc_buffer_size = bit_rate / fps`（一帧的比特数）。
/// - 「CBR」→ `bit_rate` 与 `rc_max_rate` 取同一个值。
pub fn context_settings(cfg: &EncodeConfig) -> ContextSettings {
    let fps = cfg.fps.max(1);
    let bit_rate = i64::from(cfg.bitrate_bps);
    // VBV = 1 帧：一帧的比特数。
    let rc_buffer_size = (bit_rate / i64::from(fps)).clamp(1, i64::from(i32::MAX)) as i32;
    // 兜底的 IDR 间隔：4 秒。若调用方显式给了 `idr_interval` 则以它为准。
    let gop_size = if cfg.idr_interval > 0 {
        cfg.idr_interval as i32
    } else {
        (fps * 4) as i32
    };
    ContextSettings {
        bit_rate,
        rc_max_rate: bit_rate,
        rc_buffer_size,
        max_b_frames: 0,
        gop_size,
        time_base: (1, fps as i32),
        framerate: (fps as i32, 1),
        low_delay: true,
    }
}

/// §3.6 / §6：把一段编码后的码流切成 `n` 片。
///
/// `EncodedFrame.slices` 是**传输层**切片（协议层用 `Bytes` 共享，零拷贝），
/// 与编码器内部的 slice（`AVCodecContext.slice_count`，软件编码器才有）无关。
///
/// - `n <= 1` → 不切，原样返回一片。
/// - `n > 1` → 切成 `n` 段，各段长度尽量相等（前面的段多 1 字节）。
pub fn split_into_slices(payload: bytes::Bytes, n: u32) -> Vec<bytes::Bytes> {
    let total = payload.len();
    let n = n as usize;
    if n <= 1 || total == 0 {
        return vec![payload];
    }
    let base = total / n;
    let rem = total % n;
    let mut out = Vec::with_capacity(n);
    let mut offset = 0usize;
    for i in 0..n {
        let len = base + usize::from(i < rem);
        out.push(payload.slice(offset..offset + len));
        offset += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> EncodeConfig {
        EncodeConfig {
            encoder: EncoderKind::Nvenc,
            codec: CodecKind::Hevc,
            bitrate_bps: 20_000_000,
            fps: 60,
            slices: 0,
            sfe: false,
            idr_interval: 0,
        }
    }

    fn opt<'a>(opts: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        opts.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn nvenc_baseline_matches_doc() {
        let opts = baseline_options(EncoderKind::Nvenc, &cfg());
        assert_eq!(opt(&opts, "preset"), Some("p1"), "§3.6 要求 NVENC P1");
        assert_eq!(
            opt(&opts, "tune"),
            Some("ull"),
            "§3.6 要求 ultra-low-latency"
        );
        assert_eq!(opt(&opts, "rc"), Some("cbr"), "§3.6 要求 CBR");
        assert_eq!(
            opt(&opts, "rc-lookahead"),
            Some("0"),
            "§3.6 要求无 lookahead"
        );
        assert_eq!(
            opt(&opts, "intra-refresh"),
            Some("1"),
            "§3.6 要求 intra-refresh"
        );
        assert!(opt(&opts, "forced-idr").is_some(), "需要强制 IDR");
        assert!(
            opt(&opts, "split_mode").is_none(),
            "未开 SFE 时不得下发 split_mode"
        );
    }

    #[test]
    fn nvenc_sfe_adds_split_mode() {
        let mut c = cfg();
        c.sfe = true;
        let opts = baseline_options(EncoderKind::Nvenc, &c);
        assert_eq!(
            opt(&opts, "split_mode"),
            Some("rowcol"),
            "§3.6 要求 Ada 上开启 SFE"
        );
    }

    #[test]
    fn amf_baseline_matches_doc() {
        let opts = baseline_options(EncoderKind::Amf, &cfg());
        assert_eq!(opt(&opts, "usage"), Some("ultralowlatency"));
        assert_eq!(opt(&opts, "rc"), Some("vbr_latency"));
    }

    #[test]
    fn qsv_baseline_matches_doc() {
        let opts = baseline_options(EncoderKind::Qsv, &cfg());
        assert_eq!(opt(&opts, "preset"), Some("veryfast"));
        assert_eq!(opt(&opts, "look_ahead"), Some("0"));
        assert_eq!(opt(&opts, "bf"), Some("0"));
    }

    #[test]
    fn auto_has_no_vendor_options() {
        assert!(baseline_options(EncoderKind::Auto, &cfg()).is_empty());
    }

    #[test]
    fn context_settings_encode_doc_baseline() {
        let s = context_settings(&cfg());
        assert_eq!(s.max_b_frames, 0, "§3.6 要求无 B 帧");
        assert_eq!(s.bit_rate, 20_000_000);
        assert_eq!(s.rc_max_rate, 20_000_000, "CBR 下上限等于目标码率");
        assert_eq!(s.rc_buffer_size, 20_000_000 / 60, "§3.6 要求 VBV = 1 帧");
        assert!(s.low_delay);
        assert_eq!(s.time_base, (1, 60));
        assert_eq!(s.framerate, (60, 1));
    }

    #[test]
    fn encoder_names_cover_vendor_matrix() {
        assert_eq!(
            encoder_name(EncoderKind::Nvenc, CodecKind::Hevc),
            Some("hevc_nvenc")
        );
        assert_eq!(
            encoder_name(EncoderKind::Amf, CodecKind::H264),
            Some("h264_amf")
        );
        assert_eq!(
            encoder_name(EncoderKind::Qsv, CodecKind::Av1),
            Some("av1_qsv")
        );
        assert_eq!(encoder_name(EncoderKind::Auto, CodecKind::Hevc), None);
    }

    #[test]
    fn slices_are_even_and_lossless() {
        let payload = bytes::Bytes::from((0u8..=99).collect::<Vec<u8>>());
        let one = split_into_slices(payload.clone(), 0);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0], payload);

        let four = split_into_slices(payload.clone(), 4);
        assert_eq!(four.len(), 4);
        assert_eq!(four.iter().map(|s| s.len()).sum::<usize>(), payload.len());
        assert_eq!(four.concat(), payload, "切片必须无损");
        // 前面的段多 1 字节。
        assert_eq!(four[0].len(), 25);
        assert_eq!(four[3].len(), 25);
    }

    #[test]
    fn droppable_order_never_drops_core_baseline() {
        for core in ["preset", "usage", "rc"] {
            assert!(
                !DROPPABLE_OPTIONS.contains(&core),
                "{core} 是基线核心，不得降档"
            );
        }
    }
}
