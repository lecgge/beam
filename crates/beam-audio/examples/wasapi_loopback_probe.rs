//! 真机 WASAPI 回环验证（§3.7）。
//!
//! ## 为什么需要它
//!
//! §3.7 要求「主机 WASAPI loopback 采集 → Opus 编码 → 客户端 Opus 解码 → WASAPI 输出」。
//! 音频这一段此前**没有任何真机证据**（缺一整条链路的验证入口），
//! 而「无声卡 / 无回环」曾长期是本项目的阻塞项。
//!
//! 本入口把链路的**两端**放在同一台机器上跑：
//!
//! 1. 打开默认渲染端点（`WasapiAudioOutput`）；
//! 2. 打开默认采集端点的 loopback（`WasapiLoopbackSource`）；
//! 3. 播一段 440 Hz 正弦波（**真的出声**，用户能听到）；
//! 4. 把采回来的 PCM 算 RMS —— 静音的 RMS 是 0，
//!    所以这一步能证明「采到的是真声音，不是空数据」。
//!
//! ## 判据
//!
//! - 渲染/采集端点都开得起来；
//! - 采到 ≥ 1 帧；
//! - 采到的 PCM **非静音**（RMS > 0）。
//!
//! 用法：
//!
//! ```powershell
//! cargo run -p beam-audio --example wasapi_loopback_probe --release
//! ```
//!
//! 注意：WASAPI 是**纯 Rust**（windows 绑定），这个入口不需要 ffmpeg。

use std::time::{Duration, Instant};

use beam_audio::{
    AudioConfig, AudioMode, AudioOutput, AudioSource, PcmFrame, WasapiAudioOutput,
    WasapiLoopbackSource, SAMPLE_RATE,
};

fn main() {
    println!("=== 真机 WASAPI 回环验证（§3.7）===");

    let cfg = AudioConfig {
        mode: AudioMode::HostAndStream,
        bitrate_bps: 128_000,
        sample_rate: SAMPLE_RATE,
        channels: 2,
    };

    // ---- 渲染端点（出声）----
    let mut output = match WasapiAudioOutput::new(cfg.clone()) {
        Ok(o) => o,
        Err(e) => {
            println!("FAIL: 打开渲染端点失败 {} — {e}", e.code_str());
            std::process::exit(2);
        }
    };
    println!("渲染端点: 打开成功（出声设备就绪）");

    // ---- 采集端点的 loopback（回环）----
    let mut source = WasapiLoopbackSource::new();
    if let Err(e) = source.start(cfg.clone()) {
        println!("FAIL: 打开 loopback 采集失败 {} — {e}", e.code_str());
        std::process::exit(2);
    }
    println!("loopback 采集: 打开成功");

    // ---- 播正弦波，同时采回来 ----
    //
    // 5 ms 一帧、48 kHz、双声道 ⇒ 每帧 480 个采样。
    let samples_per_frame = (SAMPLE_RATE as usize / 1000) * 5;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut frames_captured = 0usize;
    let mut max_rms = 0.0f64;
    let mut next_emit = Instant::now();

    while Instant::now() < deadline {
        // 播放：440 Hz 正弦波（**真的出声**）。
        if Instant::now() >= next_emit {
            next_emit = Instant::now() + Duration::from_millis(5);
            let mut samples = Vec::with_capacity(samples_per_frame * 2);
            for i in 0..samples_per_frame {
                let t = i as f64 / SAMPLE_RATE as f64;
                let v = (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 0.2;
                let s = (v * i16::MAX as f64) as i16;
                samples.extend_from_slice(&s.to_le_bytes());
                samples.extend_from_slice(&s.to_le_bytes());
            }
            let pcm = PcmFrame {
                samples: bytes::Bytes::from(samples),
                sample_rate: SAMPLE_RATE,
                channels: 2,
            };
            if let Err(e) = output.play(&pcm) {
                println!("FAIL: 播放失败 {} — {e}", e.code_str());
                std::process::exit(1);
            }
        }

        // 采集：loopback 采「系统正在播放的声音」⇒ 应当采到刚才播的正弦波。
        if let Some(frame) = source.next_frame() {
            frames_captured += 1;
            let rms = rms_i16(&frame.samples);
            if rms > max_rms {
                max_rms = rms;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    source.stop();
    // 渲染端点没有 `stop` —— `Drop` 即关闭（`WasapiAudioOutput` 的 `Drop` 会关掉输出流）。

    println!("采到的 PCM 帧数 = {}", frames_captured);
    println!("采到内容的峰值 RMS = {max_rms:.1}（0 = 静音）");
    if frames_captured == 0 {
        println!("RESULT: FAIL —— loopback 一帧都没采到");
        std::process::exit(1);
    }
    if max_rms <= 0.0 {
        println!("RESULT: FAIL —— 采到的是静音，不是真声音");
        std::process::exit(1);
    }
    println!("RESULT: PASS —— WASAPI 回环真的采到了正在播放的声音");
}

/// 16 位小端 PCM 的 RMS。
fn rms_i16(bytes: &[u8]) -> f64 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for chunk in bytes.chunks_exact(2) {
        let v = i16::from_le_bytes([chunk[0], chunk[1]]) as f64 / i16::MAX as f64;
        sum += v * v;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    (sum / count as f64).sqrt()
}
