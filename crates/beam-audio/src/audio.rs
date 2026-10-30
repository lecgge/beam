//! 音频契约与默认实现（§3.7）。
//!
//! - 主机：WASAPI loopback 48 kHz → Opus 5 ms 帧；
//! - Windows 客户端：Opus 解码 + WASAPI 输出；
//! - Android：由 `core-audio` 解码（可复用 FFI 中的 Opus 解码）。

use std::time::Duration;

use bytes::Bytes;

use crate::error::AudioError;

/// §3.7 采样率。
pub const SAMPLE_RATE: u32 = 48_000;

/// §3.7 Opus 帧长：5 ms。
pub const FRAME_DURATION_MS: u32 = 5;

/// §3.7 单声道 Opus 5 ms 帧的采样数。
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as usize * FRAME_DURATION_MS as usize) / 1000;

/// §3.7 默认码率。
pub const DEFAULT_BITRATE_BPS: u32 = 128_000;

/// 音频模式（§3.7 `audio_mode=host+stream|stream_only|off`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioMode {
    /// 主机采集并串流。
    HostAndStream,
    /// 只串流（主机不采集）。
    StreamOnly,
    /// 关闭。
    #[default]
    Off,
}

/// 音频配置（§3.7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    /// 音频模式。
    pub mode: AudioMode,
    /// 码率，bit/s。
    pub bitrate_bps: u32,
    /// 采样率。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: AudioMode::Off,
            bitrate_bps: DEFAULT_BITRATE_BPS,
            sample_rate: SAMPLE_RATE,
            channels: 2,
        }
    }
}

/// 一帧 PCM 音频（交错采样）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame {
    /// 采样数据。
    pub samples: Bytes,
    /// 采样率。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
}

/// 主机侧音频采集（WASAPI loopback）。
pub trait AudioSource: Send {
    /// 开始采集。
    fn start(&mut self, cfg: AudioConfig) -> Result<(), AudioError>;
    /// 取下一个 PCM 帧；暂无数据返回 `None`。
    fn next_frame(&mut self) -> Option<PcmFrame>;
    /// 停止采集。
    fn stop(&mut self);
}

/// Opus 编码器。
pub trait OpusEncoder: Send {
    /// 编码一帧 PCM。
    fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError>;
    /// 重置编码器状态（重连时使用）。
    fn reset(&mut self);
}

/// Opus 解码器。
pub trait OpusDecoder: Send {
    /// 解码一帧 Opus。
    fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError>;
    /// 重置解码器状态。
    fn reset(&mut self);
}

/// 客户端侧音频输出（WASAPI）。
pub trait AudioOutput: Send {
    /// 播放一帧 PCM。
    fn play(&mut self, pcm: &PcmFrame) -> Result<(), AudioError>;
}

/// 默认采集源：Windows 上是 **WASAPI loopback**（采「系统正在播放的声音」）。
///
/// - Windows：真机实现（见 [`crate::wasapi`]）。设备打不开 ⇒ `E7001`。
/// - 其它平台：没有原生后端 ⇒ `E7004`。
///
/// `next_frame()` 是**非阻塞**语义：没有攒够 5 ms 就返回 `None`，
/// 调用方自己决定轮询节奏（不要在采集侧 spin）。
#[derive(Default)]
pub struct WasapiLoopbackSource {
    inner: Option<crate::wasapi::LoopbackInner>,
    cfg: Option<AudioConfig>,
    running: bool,
}

impl std::fmt::Debug for WasapiLoopbackSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasapiLoopbackSource")
            .field("backend", &self.inner.is_some())
            .field("cfg", &self.cfg)
            .field("running", &self.running)
            .finish()
    }
}

impl WasapiLoopbackSource {
    /// 新建采集源。
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否正在运行。
    pub fn is_running(&self) -> bool {
        self.running
    }
}

impl AudioSource for WasapiLoopbackSource {
    fn start(&mut self, cfg: AudioConfig) -> Result<(), AudioError> {
        let inner = crate::wasapi::LoopbackInner::open(cfg.clone())?;
        self.inner = Some(inner);
        self.cfg = Some(cfg);
        self.running = true;
        Ok(())
    }

    fn next_frame(&mut self) -> Option<PcmFrame> {
        self.inner.as_mut().and_then(|inner| inner.next_frame())
    }

    fn stop(&mut self) {
        // 关掉 inner 就会停掉采集线程（`close` 内部 join）。
        if let Some(inner) = self.inner.take() {
            inner.close();
        }
        self.running = false;
        self.cfg = None;
    }
}

/// 客户端侧音频输出：Windows 上是 **WASAPI**。
///
/// 写缓冲在**专用线程**里做（§6「播放用专用 OS 线程」）：
/// `play()` 只投递命令并等回执，所以写失败能如实返回 `E7003`，
/// 而不是「投进去就不管了」。
pub struct WasapiAudioOutput {
    inner: Option<crate::wasapi::OutputInner>,
}

impl std::fmt::Debug for WasapiAudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasapiAudioOutput")
            .field("backend", &self.inner.is_some())
            .finish()
    }
}

impl WasapiAudioOutput {
    /// 打开默认渲染端点（Windows）或返回 `E7004`（其它平台）。
    ///
    /// 设备打不开 ⇒ `E7001`；设备混音格式没法协商（既不能直接用 i16，
    /// 也转不了）⇒ `E7003`，原因写清楚了。
    pub fn new(cfg: AudioConfig) -> Result<Self, AudioError> {
        Ok(Self {
            inner: Some(crate::wasapi::OutputInner::open(cfg)?),
        })
    }

    /// 是否已打开输出端。
    pub fn is_open(&self) -> bool {
        self.inner.is_some()
    }

    /// 关闭输出端（停止写缓冲线程）。
    pub fn close(&mut self) {
        if let Some(inner) = self.inner.take() {
            inner.close();
        }
    }
}

impl AudioOutput for WasapiAudioOutput {
    fn play(&mut self, pcm: &PcmFrame) -> Result<(), AudioError> {
        match self.inner.as_mut() {
            Some(inner) => inner.play(pcm),
            None => Err(AudioError::BackendDisabled),
        }
    }
}

/// 默认 Opus 编码器。
///
/// 真正的编解码在 [`crate::opus::EncoderCore`] 里（ffmpeg 的 libopus 编码器）；
/// feature 未开启时 `encode` 返回 `E7004`（后端未启用）。
#[derive(Debug)]
pub struct OpusEncoderImpl {
    core: Option<crate::opus::EncoderCore>,
}

impl OpusEncoderImpl {
    /// 新建编码器。
    ///
    /// feature 未开启时返回 `E7004`。
    pub fn new(cfg: AudioConfig) -> Result<Self, AudioError> {
        Ok(Self {
            core: Some(crate::opus::EncoderCore::new(cfg)?),
        })
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&AudioConfig> {
        self.core.as_ref().and_then(|core| core.config())
    }
}

impl Default for OpusEncoderImpl {
    /// 默认（无后端）编码器：调用 `encode` 会得到 `E7004`。
    fn default() -> Self {
        Self { core: None }
    }
}

impl OpusEncoder for OpusEncoderImpl {
    fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError> {
        match self.core.as_mut() {
            Some(core) => core.encode(pcm),
            None => Err(AudioError::BackendDisabled),
        }
    }

    fn reset(&mut self) {
        if let Some(core) = self.core.as_mut() {
            core.reset();
        }
    }
}

/// 默认 Opus 解码器。
///
/// 真正的解码在 [`crate::opus::DecoderCore`] 里（ffmpeg 的 libopus 解码器）；
/// feature 未开启时 `decode` 返回 `E7004`（后端未启用）。
#[derive(Debug)]
pub struct OpusDecoderImpl {
    core: Option<crate::opus::DecoderCore>,
}

impl OpusDecoderImpl {
    /// 新建解码器。
    ///
    /// feature 未开启时返回 `E7004`。
    pub fn new(cfg: AudioConfig) -> Result<Self, AudioError> {
        Ok(Self {
            core: Some(crate::opus::DecoderCore::new(cfg)?),
        })
    }

    /// 当前配置。
    pub fn config(&self) -> Option<&AudioConfig> {
        self.core.as_ref().and_then(|core| core.config())
    }
}

impl Default for OpusDecoderImpl {
    /// 默认（无后端）解码器：调用 `decode` 会得到 `E7004`。
    fn default() -> Self {
        Self { core: None }
    }
}

impl OpusDecoder for OpusDecoderImpl {
    fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError> {
        match self.core.as_mut() {
            Some(core) => core.decode(packet),
            None => Err(AudioError::BackendDisabled),
        }
    }

    fn reset(&mut self) {
        if let Some(core) = self.core.as_mut() {
            core.reset();
        }
    }
}

/// 端到端音频延迟预算（§3.7：端到端目标 ≤40 ms）。
pub const END_TO_END_BUDGET_MS: u32 = 40;

/// 采集 → 编码预算（§3.7：≤6 ms）。
pub const CAPTURE_TO_ENCODE_BUDGET_MS: u32 = 6;

/// 一帧的时长。
pub fn frame_duration() -> Duration {
    Duration::from_millis(u64::from(FRAME_DURATION_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_doc() {
        let cfg = AudioConfig::default();
        assert_eq!(cfg.mode, AudioMode::Off);
        assert_eq!(cfg.bitrate_bps, 128_000);
        assert_eq!(cfg.sample_rate, 48_000);
        assert_eq!(SAMPLES_PER_FRAME, 240);
    }

    #[test]
    fn capture_source_is_idle_before_start() {
        let mut src = WasapiLoopbackSource::new();
        assert!(!src.is_running());
        // 没启动就没有帧（非阻塞语义）。
        assert!(src.next_frame().is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn capture_on_non_windows_reports_e7004() {
        let mut src = WasapiLoopbackSource::new();
        let err = src.start(AudioConfig::default()).expect_err("必须失败");
        assert_eq!(err.code(), 7004);
    }

    #[cfg(not(windows))]
    #[test]
    fn output_on_non_windows_reports_e7004() {
        let err = WasapiAudioOutput::new(AudioConfig::default()).expect_err("必须失败");
        assert_eq!(err.code(), 7004);
    }

    /// 真机测试的开关：除了 `#[ignore]` 之外还要这个环境变量，
    /// 免得有人一把 `--ignored` 就把设备测试全跑了。
    #[cfg(windows)]
    fn device_test_enabled() -> bool {
        std::env::var("BEAM_AUDIO_DEVICE_TEST").as_deref() == Ok("1")
    }

    /// 真机测试：打开 loopback 采集，看能不能拿到 5 ms 帧。
    ///
    /// 默认**不跑**；要跑得显式开：
    /// `BEAM_AUDIO_DEVICE_TEST=1 cargo test -p beam-audio -- --ignored`
    ///
    /// 注意：系统没在播声音时**一帧都没有**是正常的（loopback 只采渲染流），
    /// 所以这里不把「没拿到帧」当失败。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要真实音频设备：BEAM_AUDIO_DEVICE_TEST=1 cargo test -p beam-audio -- --ignored"]
    fn wasapi_loopback_captures_5ms_frames() {
        if !device_test_enabled() {
            eprintln!("跳过：需要 BEAM_AUDIO_DEVICE_TEST=1");
            return;
        }
        let mut src = WasapiLoopbackSource::new();
        src.start(AudioConfig {
            mode: AudioMode::HostAndStream,
            ..AudioConfig::default()
        })
        .expect("应当能打开 loopback 设备");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut got = 0usize;
        while std::time::Instant::now() < deadline && got < 4 {
            match src.next_frame() {
                Some(frame) => {
                    assert_eq!(frame.sample_rate, 48_000);
                    assert_eq!(frame.channels, 2);
                    // §3.7：5 ms = 240 采样 @ 48 kHz，双声道交错 i16。
                    assert_eq!(frame.samples.len(), 240 * 2 * 2);
                    got += 1;
                }
                None => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        src.stop();
        eprintln!("loopback 取到 {got} 帧（设备在播放时才会有帧）");
    }

    /// 真机测试：输出端**只写静音**，不会发出任何可听声音。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要真实音频设备：BEAM_AUDIO_DEVICE_TEST=1 cargo test -p beam-audio -- --ignored"]
    fn wasapi_output_accepts_a_silent_frame() {
        if !device_test_enabled() {
            eprintln!("跳过：需要 BEAM_AUDIO_DEVICE_TEST=1");
            return;
        }
        let mut out = WasapiAudioOutput::new(AudioConfig::default()).expect("应当能打开输出设备");
        assert!(out.is_open());
        let pcm = PcmFrame {
            samples: Bytes::from(vec![0u8; 240 * 2 * 2]),
            sample_rate: 48_000,
            channels: 2,
        };
        // 全零 ⇒ 静音，不会打扰用户。
        out.play(&pcm).expect("写静音帧应当成功");
    }

    #[test]
    fn codec_without_backend_reports_error() {
        let mut enc = OpusEncoderImpl::default();
        let pcm = PcmFrame {
            samples: Bytes::from_static(&[0u8; 960]),
            sample_rate: 48_000,
            channels: 2,
        };
        let err = enc.encode(&pcm).expect_err("必须失败");
        assert_eq!(err.code(), 7004);
    }
}
