//! 真机音频流水线**速率**对照实验（§3.7）。
//!
//! ## 为什么需要它
//!
//! 整链验证（B20）跑通后，日志里刷出「采集队列积压，丢弃最旧的一帧」，
//! 而主机实际只送出 65 帧/秒 —— §3.7 的音频帧是 **5 ms 一帧**，
//! 实时需要 **200 帧/秒**。也就是说音频流丢了三分之二。
//!
//! 这类「跑通了但速率不对」的缺陷**测不出来**（帧数 > 0 就判定通过），
//! 所以这里专门测速率。
//!
//! ## 三段对照
//!
//! | 段 | 测的是什么 | 期望 |
//! |---|---|---|
//! | A | 只从 loopback 取帧（不编码） | 200 帧/秒（实时） |
//! | B | 取帧 + **Opus 编码** | 200 帧/秒 |
//! | C | 取帧 + Opus 编码 + **建数据报** | 200 帧/秒 |
//!
//! 哪一段掉下来，瓶颈就在那一段 —— 不猜，只测。
//!
//! 用法：
//!
//! ```powershell
//! $env:FFMPEG_DIR = "$env:TEMP\beam-tools\ffmpeg-lgpl-conda\root\Library"
//! $env:LIBCLANG_PATH = "$env:TEMP\beam-tools\libclang\libclang-18.1.1.data\platlib\clang\native"
//! $env:PATH = "$env:FFMPEG_DIR\bin;$env:PATH"
//! cargo run -p beam-audio --example audio_rate_probe --features opus --release
//! ```

use std::time::{Duration, Instant};

use beam_audio::{
    wire, AudioConfig, AudioMode, AudioOutput, AudioSource, OpusEncoder, OpusEncoderImpl, PcmFrame,
    WasapiAudioOutput, WasapiLoopbackSource, SAMPLE_RATE,
};

/// §3.7：5 ms 一帧 ⇒ 实时需要 200 帧/秒。
const REAL_TIME_FPS: f64 = 200.0;
/// 每段测多久。
const MEASURE_SECS: u64 = 3;
/// 5 ms 一帧、48 kHz、双声道 ⇒ 每帧 480 个采样（交错 i16）。
const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as usize / 1000) * 5;

fn main() {
    println!("=== 真机音频流水线速率对照（§3.7，实时需要 {REAL_TIME_FPS:.0} 帧/秒）===");

    let cfg = AudioConfig {
        mode: AudioMode::HostAndStream,
        bitrate_bps: 128_000,
        sample_rate: SAMPLE_RATE,
        channels: 2,
    };

    // ---- 渲染端点（出声）：loopback 只采「渲染流」，系统静音就采不到东西 ----
    let mut output = match WasapiAudioOutput::new(cfg.clone()) {
        Ok(o) => o,
        Err(e) => {
            println!("FAIL: 打开渲染端点失败 {} — {e}", e.code_str());
            std::process::exit(2);
        }
    };
    println!("渲染端点: 打开成功");

    // ---- A 段：只取帧 ----
    let rate_a = measure(
        "A 只取帧（不编码）",
        cfg.clone(),
        |_pcm| Ok(()),
        &mut output,
    );

    // ---- B 段：取帧 + Opus 编码 ----
    let mut encoder = match OpusEncoderImpl::new(cfg.clone()) {
        Ok(e) => e,
        Err(e) if e.code() == 7004 => {
            println!("SKIP: 没开 `opus` feature（后端未启用）—— 用 `--features opus` 再跑");
            std::process::exit(3);
        }
        Err(e) => {
            println!("FAIL: 打开 Opus 编码器失败 {} — {e}", e.code_str());
            std::process::exit(2);
        }
    };
    let rate_b = measure(
        "B 取帧 + Opus 编码",
        cfg.clone(),
        |pcm| encoder.encode(pcm).map(|_| ()),
        &mut output,
    );

    // ---- C 段：取帧 + Opus 编码 + 建数据报 ----
    let mut encoder = match OpusEncoderImpl::new(cfg.clone()) {
        Ok(e) => e,
        Err(e) => {
            println!("FAIL: 打开 Opus 编码器失败 {} — {e}", e.code_str());
            std::process::exit(2);
        }
    };
    let rate_c = measure(
        "C 取帧 + 编码 + 建数据报",
        cfg.clone(),
        |pcm| {
            let opus = encoder.encode(pcm)?;
            wire::encode_audio_datagram(0, 0, &opus).map(|_| ())
        },
        &mut output,
    );

    println!("--------------------------------------------------");
    println!("A 只取帧           : {rate_a:.1} 帧/秒");
    println!("B +Opus 编码       : {rate_b:.1} 帧/秒");
    println!("C +建音频数据报    : {rate_c:.1} 帧/秒");

    let slowest = rate_a.min(rate_b).min(rate_c);
    if slowest >= REAL_TIME_FPS * 0.95 {
        println!("RESULT: PASS —— 三段都能跟上实时（≥ {REAL_TIME_FPS:.0} 帧/秒）");
    } else {
        println!(
            "RESULT: FAIL —— 最慢的一段只有 {slowest:.1} 帧/秒，达不到实时的 {REAL_TIME_FPS:.0}"
        );
        std::process::exit(1);
    }
}

/// 测一段流水线的速率：开 loopback → 边播测试音边消费 → 统计消费帧数。
///
/// `work` 是每帧要做的额外工作（编码 / 建数据报）；返回 `Err` 表示这一帧失败。
fn measure<F>(name: &str, cfg: AudioConfig, mut work: F, output: &mut WasapiAudioOutput) -> f64
where
    F: FnMut(&PcmFrame) -> Result<(), beam_audio::AudioError>,
{
    let mut source = WasapiLoopbackSource::new();
    if let Err(e) = source.start(cfg) {
        println!(
            "FAIL: [{name}] 打开 loopback 采集失败 {} — {e}",
            e.code_str()
        );
        std::process::exit(2);
    }

    let start = Instant::now();
    let deadline = start + Duration::from_secs(MEASURE_SECS);
    let mut consumed = 0u64;
    let mut next_emit = Instant::now();
    let mut phase = 0f32;
    let step = 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;

    while Instant::now() < deadline {
        // 播测试音（每 5 ms 一帧），让 loopback 采得到内容。
        if Instant::now() >= next_emit {
            next_emit = Instant::now() + Duration::from_millis(5);
            let mut samples = Vec::with_capacity(SAMPLES_PER_FRAME * 2 * 2);
            for _ in 0..SAMPLES_PER_FRAME {
                let value = (0.2 * phase.sin() * i16::MAX as f32) as i16;
                samples.extend_from_slice(&value.to_le_bytes());
                samples.extend_from_slice(&value.to_le_bytes());
                phase += step;
            }
            let pcm = PcmFrame {
                samples: bytes::Bytes::from(samples),
                sample_rate: SAMPLE_RATE,
                channels: 2,
            };
            if let Err(e) = output.play(&pcm) {
                println!("FAIL: [{name}] 播放失败 {} — {e}", e.code_str());
                std::process::exit(2);
            }
        }

        // 消费：**一次把队列里攒下的帧全取走**（这正是真实流水线该有的行为），
        // 否则消费速率只要低于实时速率，队列就会一路涨到爆。
        while let Some(frame) = source.next_frame() {
            if work(&frame).is_err() {
                println!("FAIL: [{name}] 处理帧失败");
                std::process::exit(1);
            }
            consumed += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    source.stop();
    let elapsed = start.elapsed().as_secs_f64();
    let rate = consumed as f64 / elapsed;
    println!("  [{name}] 消费 {consumed} 帧 / {elapsed:.2} 秒 = {rate:.1} 帧/秒");
    rate
}
