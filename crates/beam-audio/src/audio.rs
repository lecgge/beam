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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    /// 主机采集并串流。
    HostAndStream,
    /// 只串流（主机不采集）。
    StreamOnly,
    /// 关闭。
    Off,
}

impl Default for AudioMode {
    fn default() -> Self {
        AudioMode::Off
    }
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

/// 默认采集源（未启用原生音频后端时返回 `E7004`）。
#[derive(Debug, Default)]
pub struct WasapiLoopbackSource {
    cfg: Option<AudioConfig>,
    running: bool,
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
        #[cfg(feature = "opus")]
        {
            let _ = cfg;
            self.cfg = Some(cfg);
            self.running = true;
            Ok(())
        }
        #[cfg(not(feature = "opus"))]
        {
            let _ = cfg;
            Err(AudioError::BackendDisabled)
        }
    }

    fn next_frame(&mut self) -> Option<PcmFrame> {
        None
    }

    fn stop(&mut self) {
        self.running = false;
        self.cfg = None;
    }
}

/// 默认 Opus 编码器（未启用原生后端时返回 `E7004`）。
#[derive(Debug, Default)]
pub struct OpusEncoderImpl {
    cfg: Option<AudioConfig>,
}

impl OpusEncoderImpl {
    /// 当前配置。
    pub fn config(&self) -> Option<&AudioConfig> {
        self.cfg.as_ref()
    }
}

impl OpusEncoder for OpusEncoderImpl {
    fn encode(&mut self, pcm: &PcmFrame) -> Result<Bytes, AudioError> {
        let _ = pcm;
        Err(AudioError::BackendDisabled)
    }

    fn reset(&mut self) {}
}

/// 默认 Opus 解码器（未启用原生后端时返回 `E7004`）。
#[derive(Debug, Default)]
pub struct OpusDecoderImpl {
    cfg: Option<AudioConfig>,
}

impl OpusDecoderImpl {
    /// 当前配置。
    pub fn config(&self) -> Option<&AudioConfig> {
        self.cfg.as_ref()
    }
}

impl OpusDecoder for OpusDecoderImpl {
    fn decode(&mut self, packet: &[u8]) -> Result<PcmFrame, AudioError> {
        let _ = packet;
        Err(AudioError::BackendDisabled)
    }

    fn reset(&mut self) {}
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
    fn capture_without_backend_reports_e7004() {
        let mut src = WasapiLoopbackSource::new();
        #[cfg(not(feature = "opus"))]
        {
            let err = src.start(AudioConfig::default()).expect_err("必须失败");
            assert_eq!(err.code(), 7004);
        }
        #[cfg(feature = "opus")]
        {
            src.start(AudioConfig::default()).expect("应当成功");
            assert!(src.is_running());
        }
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
