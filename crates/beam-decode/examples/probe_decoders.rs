//! 验证入口：列出本机实际可用的硬件解码器（§3.10）。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR    = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-decode --example probe_decoders --features ffmpeg
//! ```
//!
//! 输出说明：
//!
//! - `d3d11va=yes` 表示该解码器在 ffmpeg 里声明了 `AV_PIX_FMT_D3D11` 硬件配置，
//!   并且支持 `AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX`（本实现采用的路线）。
//! - `d3d11va=no` 表示只能软解——此时本 crate 的硬解路径不可用。
//!
//! **说明**：这里的输出反映「ffmpeg 构建 + 驱动声明」，
//! 不代表真的能解出画面——本批没有送入任何真实码流。

fn main() {
    println!("=== beam-decode 解码器探测 ===");
    println!("ffmpeg feature : {}", cfg!(feature = "ffmpeg"));
    println!("av1  feature   : {}", cfg!(feature = "av1"));
    println!();

    #[cfg(not(feature = "ffmpeg"))]
    {
        println!("本次构建没有开启 `ffmpeg` feature，不会触碰原生库。");
        println!("启用后重跑：cargo run -p beam-decode --example probe_decoders --features ffmpeg");
    }

    #[cfg(feature = "ffmpeg")]
    {
        use beam_decode::ffmpeg::probe_decoders;

        let caps = probe_decoders();
        if caps.is_empty() {
            println!("本机 ffmpeg 构建里没有可用的 HEVC / H.264 解码器。");
            println!("含义：{}", beam_decode::message(10001));
            return;
        }

        println!("本机可用解码器 {} 个：", caps.len());
        for c in &caps {
            println!(
                "  - {:<5} {:<6} d3d11va={} max={}x{} @{}fps",
                codec_name(c.codec),
                c.name,
                if c.d3d11va { "yes" } else { "no " },
                c.max_width,
                c.max_height,
                c.max_fps,
            );
        }

        let hw: Vec<_> = caps.iter().filter(|c| c.d3d11va).collect();
        println!();
        if hw.is_empty() {
            println!("没有任何解码器声明 D3D11VA 硬解，D3D11VA 路径不可用。");
        } else {
            println!(
                "D3D11VA 可用于：{}",
                hw.iter()
                    .map(|c| codec_name(c.codec))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
}

/// 编解码器的短名。
#[cfg(feature = "ffmpeg")]
fn codec_name(codec: beam_proto::CodecKind) -> &'static str {
    match codec {
        beam_proto::CodecKind::Hevc => "hevc",
        beam_proto::CodecKind::H264 => "h264",
        beam_proto::CodecKind::Av1 => "av1",
    }
}
