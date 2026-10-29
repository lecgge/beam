//! 验证入口：列出本机实际可用的硬件编码器（§3.6 `probe()`）。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-encode --example probe_encoders --features ffmpeg
//! ```
//!
//! 这个入口**真的**去枚举 ffmpeg 的 NVENC / AMF / QSV 编码器，并逐个尝试打开，
//! 因此输出反映的是「本机此刻能不能硬编」，而不是「ffmpeg 构建里有没有这个编码器」。
//!
//! **说明**：输出反映的是「本机此刻能不能硬编」，而不是「ffmpeg 构建里有没有这个编码器」。
//! 本批没有送入任何真实帧，因此**不能**据此认为出码流已经验证。

use beam_encode::params::{baseline_options, context_settings, encoder_name};
use beam_encode::{probe, EncodeConfig, EncoderKind, HardwareVideoEncoder};

fn main() {
    println!("=== beam-encode 编码器探测 ===");
    println!("ffmpeg feature : {}", cfg!(feature = "ffmpeg"));
    println!("av1  feature   : {}", cfg!(feature = "av1"));
    println!();

    let caps = probe();
    if caps.is_empty() {
        println!("本机没有可用的硬件编码器（probe() 返回空列表）。");
        println!("含义：{}", beam_encode::message(6001));
        if !cfg!(feature = "ffmpeg") {
            println!("提示：本次构建没有开启 `ffmpeg` feature，probe() 不会触碰原生库。");
        } else {
            println!("提示：已开启 `ffmpeg` feature，说明 ffmpeg 构建里没有 NVENC/AMF/QSV，");
            println!("      或者本机没有对应的 GPU / 驱动。");
        }
    } else {
        println!("本机可用硬件编码器 {} 个：", caps.len());
        for c in &caps {
            println!(
                "  - {:<6} {:<5} max={}x{} @{}fps low_latency={} slice={} sfe={}",
                c.encoder.as_str(),
                codec_name(c.codec),
                c.max_width,
                c.max_height,
                c.max_fps,
                c.low_latency,
                c.slice,
                c.sfe,
            );
        }
    }
    println!();

    // §3.6 的 `encoder=auto` 选择优先级。
    let picked = beam_encode::select_encoder_kind(&caps, EncoderKind::Auto);
    println!(
        "encoder=auto 选中 : {}",
        picked.map(|k| k.as_str()).unwrap_or("(无)")
    );
    println!();

    // 打印 §3.6 的参数基线，便于与文档逐条核对。
    println!("=== §3.6 参数基线 ===");
    for kind in EncoderKind::AUTO_PRIORITY {
        let cfg = EncodeConfig {
            encoder: kind,
            ..EncodeConfig::default()
        };
        println!("[{}]", kind.as_str());
        for (k, v) in baseline_options(kind, &cfg) {
            println!("  -{k:<14} {v}");
        }
        let s = context_settings(&cfg);
        println!(
            "  上下文字段: bit_rate={} rc_max_rate={} rc_buffer_size={} (VBV 1 帧) max_b_frames={} gop={} low_delay={}",
            s.bit_rate, s.rc_max_rate, s.rc_buffer_size, s.max_b_frames, s.gop_size, s.low_delay
        );
        println!();
    }

    // 真正建立一次编码器上下文，打印错误码。
    println!("=== 建立编码器上下文 ===");
    let cfg = EncodeConfig {
        encoder: EncoderKind::Auto,
        ..EncodeConfig::default()
    };
    match HardwareVideoEncoder::open(cfg) {
        Ok(_) => println!("编码器上下文建立成功。"),
        Err(e) => println!("失败：{}（{}）— {}", e.code_str(), e.code(), e.text()),
    }

    println!();
    println!("=== ffmpeg 编码器名对照 ===");
    for kind in EncoderKind::AUTO_PRIORITY {
        for codec in [beam_proto::CodecKind::Hevc, beam_proto::CodecKind::H264] {
            println!(
                "  {:<6} {:<5} -> {}",
                kind.as_str(),
                codec_name(codec),
                encoder_name(kind, codec).unwrap_or("(不支持)")
            );
        }
    }
}

/// 编解码器的短名。
fn codec_name(codec: beam_proto::CodecKind) -> &'static str {
    match codec {
        beam_proto::CodecKind::Hevc => "hevc",
        beam_proto::CodecKind::H264 => "h264",
        beam_proto::CodecKind::Av1 => "av1",
    }
}
