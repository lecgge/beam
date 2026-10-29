//! 主机运行时组装（§3.9）。

use beam_capture::{CaptureConfig, CaptureSource};
use beam_config::ResolvedConfig;
use beam_discovery::{Advertiser, HostInfo};
use beam_encode::{EncodeConfig, VideoEncoder};
use beam_input::InputSink as PlatformInputSink;
use beam_net::VideoSender as NetVideoSenderTrait;
use beam_proto::StreamConfig;
use beam_session::{
    HostControlDeps, HostDeps, HostSession, HostSessionConfig, PairingConfig, SessionError,
    SessionHandle, SessionInput, SessionTimeouts,
};

use crate::adapters::{CaptureEncodePipeline, InputAdapter, NetVideoSender, PipelineErrors};
use crate::net::{HostInput, HostTransport};

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

    /// 传输层失败（`E2xxx` / `E9xxx`）。
    #[error("传输失败: {reason}")]
    TransportUnavailable {
        /// 原因。
        reason: String,
    },
}

/// 主机配置。
#[derive(Debug, Clone, Default)]
pub struct HostConfig {
    /// 串流配置。
    pub stream: StreamConfig,
    /// 抓屏配置。
    ///
    /// 由 [`ResolvedConfig`] 派生（§3.14 → §3.9），
    /// `backend` / `display_id` / `min_update_interval` 三项**真的**送达抓屏层。
    pub capture: CaptureConfig,
    /// 编码配置。
    ///
    /// 由 [`ResolvedConfig`] 派生（§3.14 → §3.9），
    /// `slices` / `sfe` 等项由调用方用它构造编码器。
    pub encode: EncodeConfig,
    /// 本机广播信息。
    pub advertise: Option<HostInfo>,
}

impl HostConfig {
    /// 从解析后的配置生成（§3.14 的设置 → 运行参数）。
    ///
    /// 这是**单一真值来源**的最后一跳：
    /// 抓屏配置直接取 [`beam_capture::CaptureConfig`] 的 `From<&ResolvedConfig>`，
    /// 编码配置直接取 [`beam_encode::EncodeConfig`] 的 `From<&ResolvedConfig>`，
    /// 本 crate 不再自己复制一份配置项。
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
            capture: CaptureConfig::from(cfg),
            encode: EncodeConfig::from(cfg),
            advertise: None,
        }
    }
}

/// 主机侧会话（§3.9）：会话句柄 + 输入泵任务 + 流水线错误槽。
///
/// 会话状态机由 `beam-session` 驱动；输入事件由本 crate 的输入泵注入
/// （`beam-session` 的 `HostDeps::input` 目前没有消费方，
/// 因此输入必须在主机侧自己驱动）。
///
/// 编码错误同理：`beam-session` 的 `FramePipeline::poll()` 没有错误通道，
/// 真实错误由 [`HostSessionHandle::take_error`] / [`HostSessionHandle::pump`] 冒泡。
pub struct HostSessionHandle {
    /// 会话句柄（状态 / 事件）。
    pub session: SessionHandle,
    /// 流水线的真实错误槽。
    errors: PipelineErrors,
    /// 输入泵任务；句柄释放时自动取消。
    input_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for HostSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSessionHandle")
            .field("session", &self.session)
            .field("has_pipeline_error", &self.errors.has_error())
            .field("has_input_pump", &self.input_task.is_some())
            .finish()
    }
}

impl Drop for HostSessionHandle {
    fn drop(&mut self) {
        if let Some(task) = self.input_task.take() {
            task.abort();
        }
    }
}

impl HostSessionHandle {
    /// 当前会话状态（§3.9 状态机）。
    ///
    /// 直接取自 `beam-session` 的状态机，主机侧与桌面端读的是同一个来源。
    pub fn state(&self) -> beam_session::SessionState {
        self.session.state()
    }

    /// 推进一步会话，并把流水线的**真实**错误冒泡出来。
    ///
    /// `beam-session` 的 `FramePipeline::poll()` 没有错误返回通道，
    /// 因此本方法在推进会话之后检查错误槽：
    /// 有真实错误时让会话进入错误态，并把错误返回给调用方。
    ///
    /// 「编码器尚未产出帧」（`E6005`）**不是错误**，不会出现在这里。
    pub fn pump(&self) -> Result<(), SessionError> {
        self.session.pump()?;
        if let Some(err) = self.errors.take() {
            let _ = self.session.apply(SessionInput::Failed(err.code()));
            return Err(err);
        }
        Ok(())
    }

    /// 取走最近一次真实的流水线错误（不改变会话状态）。
    pub fn take_error(&self) -> Option<SessionError> {
        self.errors.take()
    }

    /// 是否有待取走的流水线错误。
    pub fn has_error(&self) -> bool {
        self.errors.has_error()
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
    ///
    /// 返回的 [`HostSessionHandle`] 带错误槽：调用方用
    /// [`HostSessionHandle::pump`] 推进会话时，真实的编码错误会冒泡出来。
    pub fn start_session<C, E, S, I>(
        &self,
        capture: C,
        encoder: E,
        sender: S,
        input: Option<I>,
    ) -> HostSessionHandle
    where
        C: CaptureSource + Send + 'static,
        E: VideoEncoder + Send + 'static,
        S: NetVideoSenderTrait + Send + 'static,
        I: PlatformInputSink + Send + 'static,
    {
        let pipeline = CaptureEncodePipeline::with_frame_size(
            capture,
            encoder,
            self.cfg.capture.clone(),
            (
                u32::from(self.cfg.stream.width),
                u32::from(self.cfg.stream.height),
            ),
        );
        let errors = pipeline.errors();
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
        HostSessionHandle {
            session: HostSession::start(session_cfg, deps),
            errors,
            input_task: None,
        }
    }

    /// 编码配置（§3.14 → §3.9）。
    ///
    /// 调用方用它构造编码器：`slices` / `sfe` 等项只在这里送达编码层。
    pub fn encoder_config(&self) -> &EncodeConfig {
        &self.cfg.encode
    }

    /// 用一条**已经建立**的 QUIC 连接启动主机会话（§5 三通道）。
    ///
    /// 通道分工：
    ///
    /// - 控制 stream 0（双向可靠）：[`HostTransport`] 承载 `ControlMsg`；
    /// - 画面（datagram）：[`NetVideoSender`] 直接用同一条连接发送；
    /// - 输入 stream（客户端 → 主机，单向可靠）：本 crate 的输入泵
    ///   用 [`HostInput`] 接收并注入 [`beam_session::InputSink`]。
    ///
    /// `HostTransport::accept` 会等到客户端在控制流上写入第一条数据才返回
    /// （QUIC 的流要收到对端数据才对本端可见），
    /// 因此调用方应当在客户端已经建链并开始握手之后调用本方法。
    pub async fn start_quic_session<C, E, I>(
        &self,
        conn: beam_net::Conn,
        capture: C,
        encoder: E,
        input: Option<I>,
    ) -> Result<HostSessionHandle, HostError>
    where
        C: CaptureSource + Send + 'static,
        E: VideoEncoder + Send + 'static,
        I: PlatformInputSink + Send + 'static,
    {
        let transport = HostTransport::accept(conn.clone()).await.map_err(|e| {
            HostError::TransportUnavailable {
                reason: format!("{e}"),
            }
        })?;

        let pipeline = CaptureEncodePipeline::new(capture, encoder, self.cfg.capture.clone());
        // 与 `start_session` 同法：先把共享错误槽取出来，再把流水线移进 `HostDeps`。
        let errors = pipeline.errors();
        let deps = HostDeps {
            pipeline: Box::new(pipeline),
            sender: Box::new(NetVideoSender::new(conn.clone())),
            // 输入由本 crate 的输入泵驱动（见下），不交给会话状态机。
            input: None,
        };
        let session_cfg = HostSessionConfig {
            stream: self.cfg.stream.clone(),
            reconnect_timeout: SessionTimeouts::default().reconnect_timeout,
            stall_threshold: SessionTimeouts::default().stall_threshold,
        };
        let control = HostControlDeps {
            control: Box::new(transport),
            pairing: PairingConfig::default(),
            rate: None,
        };
        let session = HostSession::start_with_control(session_cfg, deps, Some(control));

        // 输入泵：§5 输入 stream（客户端 → 主机）。
        let input_task = input.map(|sink| {
            let conn = conn.clone();
            let mut sink: Box<dyn beam_session::InputSink> = Box::new(InputAdapter::new(sink));
            tokio::spawn(async move {
                let mut channel = match HostInput::accept(&conn).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(reason = %e, "接受输入流失败,主机不再接收输入");
                        return;
                    }
                };
                loop {
                    match channel.next_event().await {
                        Some(Ok(ev)) => {
                            if let Err(e) = sink.apply(ev) {
                                tracing::warn!(reason = %e, "注入输入失败");
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(reason = %e, "输入流异常,停止输入泵");
                            return;
                        }
                        None => return,
                    }
                }
            })
        });

        Ok(HostSessionHandle {
            session,
            errors,
            input_task,
        })
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

    #[test]
    fn config_from_resolved_carries_capture_and_encode_settings() {
        // §3.14 → §3.9:抓屏与编码配置项必须真的送达各自的 Config 结构。
        let mut resolved = ResolvedConfig::default();
        resolved.capture.backend = "wgc".to_string();
        resolved.capture.display_id = Some(2);
        resolved.capture.min_update_interval = Some(3);
        resolved.video.slices = Some(4);
        resolved.video.sfe = true;

        let cfg = HostConfig::from_resolved(&resolved);
        assert_eq!(cfg.capture.backend, beam_capture::CaptureBackend::Wgc);
        assert_eq!(cfg.capture.display_id, Some(2));
        assert_eq!(cfg.capture.min_update_interval, Some(3));
        assert_eq!(cfg.capture.target_fps, resolved.video.fps);
        assert_eq!(cfg.encode.slices, 4);
        assert!(cfg.encode.sfe);
    }
}
