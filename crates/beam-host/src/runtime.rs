//! 主机运行时组装（§3.9）。

use beam_capture::{CaptureConfig, CaptureSource};
use beam_config::ResolvedConfig;
use beam_discovery::{Advertiser, HostInfo};
use beam_encode::VideoEncoder;
use beam_input::InputSink as PlatformInputSink;
use beam_net::VideoSender as NetVideoSenderTrait;
use beam_proto::StreamConfig;
use beam_session::{HostDeps, HostSession, HostSessionConfig, SessionHandle, SessionTimeouts};

use crate::adapters::{CaptureEncodePipeline, InputAdapter, NetVideoSender};

/// 主机编排层错误。
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// 抓屏后端不可用（`E5xxx`）。
    #[error("抓屏不可用: {reason}")]
    CaptureUnavailable {
        /// 原因。
        reason: String,
    },

    /// 编码器不可用（`E6xxx`）。
    #[error("编码器不可用: {reason}")]
    EncoderUnavailable {
        /// 原因。
        reason: String,
    },

    /// 发现服务启动失败（`E4xxx`）。
    #[error("发现服务启动失败: {reason}")]
    DiscoveryUnavailable {
        /// 原因。
        reason: String,
    },
}

/// 主机配置。
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// 串流配置。
    pub stream: StreamConfig,
    /// 抓屏配置。
    pub capture: CaptureConfig,
    /// 本机广播信息。
    pub advertise: Option<HostInfo>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            capture: CaptureConfig::default(),
            advertise: None,
        }
    }
}

impl HostConfig {
    /// 从解析后的配置生成（§3.14 的设置 → 运行参数）。
    pub fn from_resolved(cfg: &ResolvedConfig) -> Self {
        Self {
            stream: StreamConfig {
                width: cfg.video.width,
                height: cfg.video.height,
                fps: cfg.video.fps,
                bitrate_bps: cfg.video.bitrate_bps,
                codec: beam_proto::CodecKind::from_bits(match cfg.video.codec.as_str() {
                    "hevc" => 1,
                    "av1" => 2,
                    _ => 0,
                }),
                audio_enabled: cfg.audio.mode != beam_config::AudioMode::Off,
                features: 0,
            },
            capture: CaptureConfig {
                target_fps: cfg.video.fps,
                ..CaptureConfig::default()
            },
            advertise: None,
        }
    }
}

/// 主机运行时：把平台实现与会话层组装起来。
#[derive(Debug)]
pub struct HostRuntime {
    cfg: HostConfig,
    advertiser: Option<Advertiser>,
}

impl HostRuntime {
    /// 创建运行时（不启动广播）。
    pub fn new(cfg: HostConfig) -> Self {
        Self {
            cfg,
            advertiser: None,
        }
    }

    /// 启动 mDNS 广播（§3.4）。
    pub fn start_advertising(&mut self, info: HostInfo) -> Result<(), HostError> {
        let advertiser = Advertiser::start(info).map_err(|e| HostError::DiscoveryUnavailable {
            reason: format!("{}: {e}", e.code_str()),
        })?;
        self.advertiser = Some(advertiser);
        Ok(())
    }

    /// 停止 mDNS 广播。
    pub fn stop_advertising(&mut self) {
        self.advertiser = None;
    }

    /// 当前配置。
    pub fn config(&self) -> &HostConfig {
        &self.cfg
    }

    /// 组装并启动主机会话（§3.9 `HostSession::start(cfg, deps)`）。
    pub fn start_session<C, E, S, I>(
        &self,
        capture: C,
        encoder: E,
        sender: S,
        input: Option<I>,
    ) -> SessionHandle
    where
        C: CaptureSource + Send + 'static,
        E: VideoEncoder + Send + 'static,
        S: NetVideoSenderTrait + Send + 'static,
        I: PlatformInputSink + Send + 'static,
    {
        let pipeline = CaptureEncodePipeline::new(capture, encoder, self.cfg.capture.clone());
        let deps = HostDeps {
            pipeline: Box::new(pipeline),
            sender: Box::new(NetVideoSender::new(sender)),
            input: input
                .map(|i| Box::new(InputAdapter::new(i)) as Box<dyn beam_session::InputSink>),
        };
        let session_cfg = HostSessionConfig {
            stream: self.cfg.stream.clone(),
            reconnect_timeout: SessionTimeouts::default().reconnect_timeout,
            stall_threshold: SessionTimeouts::default().stall_threshold,
        };
        HostSession::start(session_cfg, deps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_capture::CapError;
    use beam_encode::EncError;
    use beam_proto::EncodedFrame;
    use bytes::Bytes;

    struct FakeCapture;
    impl CaptureSource for FakeCapture {
        fn start(&mut self, _cfg: CaptureConfig) -> Result<(), CapError> {
            Ok(())
        }
        fn latest(&mut self) -> Option<beam_capture::CapturedFrame> {
            None
        }
        fn stop(&mut self) {}
    }

    struct FakeEncoder;
    impl VideoEncoder for FakeEncoder {
        fn encode(&mut self, _f: &beam_encode::CapturedFrame) -> Result<EncodedFrame, EncError> {
            Err(EncError::NoHardwareEncoder)
        }
        fn request_idr(&mut self) {}
        fn set_bitrate(&mut self, _bps: u32) {}
    }

    struct FakeSender;
    impl NetVideoSenderTrait for FakeSender {
        fn send_frame(&self, _f: EncodedFrame) -> Result<(), beam_net::NetError> {
            Ok(())
        }
    }

    struct FakeInput;
    impl PlatformInputSink for FakeInput {
        fn apply(&mut self, _ev: beam_proto::InputEvent) -> Result<(), beam_input::InputError> {
            Ok(())
        }
    }

    #[test]
    fn runtime_starts_session() {
        let runtime = HostRuntime::new(HostConfig::default());
        let handle = runtime.start_session(FakeCapture, FakeEncoder, FakeSender, Some(FakeInput));
        assert_eq!(
            handle.state(),
            beam_session::SessionState::Idle,
            "刚创建时应处于 Idle"
        );
    }

    #[test]
    fn config_from_resolved_uses_video_settings() {
        let mut resolved = ResolvedConfig::default();
        resolved.video.width = 2560;
        resolved.video.height = 1440;
        resolved.video.fps = 144;
        let cfg = HostConfig::from_resolved(&resolved);
        assert_eq!(cfg.stream.width, 2560);
        assert_eq!(cfg.capture.target_fps, 144);
        let _ = Bytes::from_static(b"");
    }
}
