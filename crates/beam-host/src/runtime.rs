//! 主机运行时组装（§3.9）。

use beam_capture::{CaptureConfig, CaptureSource};
use beam_config::{InputSettings, ResolvedConfig};
use beam_discovery::{Advertiser, HostInfo};
use beam_encode::{EncodeConfig, VideoEncoder};
use beam_input::{InputInjector, InputSink as PlatformInputSink};
use beam_net::VideoSender as NetVideoSenderTrait;
use beam_pair::PeerPerms;
use beam_proto::StreamConfig;
use beam_session::{
    HostControlDeps, HostDeps, HostSession, HostSessionConfig, PairingConfig, SessionError,
    SessionHandle, SessionInput, SessionTimeouts,
};

use crate::adapters::{CaptureEncodePipeline, InputAdapter, NetVideoSender, PipelineErrors};
use crate::audio::{AudioStatsHandle, NetAudioSender};
use crate::input::map_input_permissions;
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
#[derive(Debug, Clone)]
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
    /// 输入设置（§3.14 → §3.9）。
    ///
    /// 这是**主机全局上限**：`allow_keyboard` / `allow_mouse` / `gamepad`
    /// 决定本机最多允许注入哪几类设备。它与协议侧的对端权限取**交集**
    /// 后才得到真正的注入权限（见 [`crate::input::map_input_permissions`]）。
    pub input: InputSettings,
    /// 音频配置（§3.7 `audio_mode=host+stream|stream_only|off`、`bitrate=128k`）。
    ///
    /// 由 [`ResolvedConfig`] 派生（§3.14 → §3.9）：`mode` 与 `bitrate_bps`
    /// 来自配置层，其余用 §3.7 的默认值（48 kHz / 双声道）。
    /// `mode = off`（默认）时本次会话**完全不发**音频。
    pub audio: beam_audio::AudioConfig,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            capture: CaptureConfig::default(),
            encode: EncodeConfig::default(),
            advertise: None,
            // `beam_config::InputSettings` **没有** `Default` 实现（它只存在于
            // `ResolvedConfig::default()` 里），因此这里取 `ResolvedConfig::default()`
            // 的值，保持「配置只有一份真值来源」。
            input: ResolvedConfig::default().input,
            // 音频默认关闭（§3.7 `audio_mode=off`）。
            audio: beam_audio::AudioConfig::default(),
        }
    }
}

impl HostConfig {
    /// 从解析后的配置生成（§3.14 的设置 → 运行参数）。
    ///
    /// 这是**单一真值来源**的最后一跳：
    /// 抓屏配置直接取 [`beam_capture::CaptureConfig`] 的 `From<&ResolvedConfig>`，
    /// 编码配置直接取 [`beam_encode::EncodeConfig`] 的 `From<&ResolvedConfig>`，
    /// 输入设置直接取 `ResolvedConfig.input`，
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
            input: cfg.input.clone(),
            // §3.7：音频模式与码率来自配置层，其余用 §3.7 的默认值。
            audio: beam_audio::AudioConfig {
                mode: match cfg.audio.mode {
                    beam_config::AudioMode::HostAndStream => beam_audio::AudioMode::HostAndStream,
                    beam_config::AudioMode::StreamOnly => beam_audio::AudioMode::StreamOnly,
                    beam_config::AudioMode::Off => beam_audio::AudioMode::Off,
                },
                bitrate_bps: cfg.audio.bitrate_bps,
                sample_rate: beam_audio::SAMPLE_RATE,
                channels: 2,
            },
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
    /// 真实注入器的统计（I-13：入队 → 注入完成的延迟）。
    ///
    /// 只有在主机编排层用了**真实注入器**时才有值；
    /// 调用方自带注入器时为 `None`。
    input_stats: Option<std::sync::Arc<beam_input::InjectorStats>>,
    /// 音频泵任务（§3.7）；句柄释放时自动取消。
    audio_task: Option<tokio::task::JoinHandle<()>>,
    /// 音频统计（§3.7）。
    ///
    /// 只有本次会话**接了音频**时才有值（`audio_mode=off` ⇒ `None`）。
    audio_stats: Option<AudioStatsHandle>,
}

impl std::fmt::Debug for HostSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSessionHandle")
            .field("session", &self.session)
            .field("has_pipeline_error", &self.errors.has_error())
            .field("has_input_pump", &self.input_task.is_some())
            .field("has_input_stats", &self.input_stats.is_some())
            .field("has_audio_pump", &self.audio_task.is_some())
            .field(
                "audio_stats",
                &self.audio_stats.as_ref().map(AudioStatsHandle::snapshot),
            )
            .finish()
    }
}

impl Drop for HostSessionHandle {
    fn drop(&mut self) {
        if let Some(task) = self.input_task.take() {
            task.abort();
        }
        if let Some(task) = self.audio_task.take() {
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

    /// 真实注入器的统计（I-13：入队 → 注入完成的延迟）。
    ///
    /// 返回 `None` 表示这次会话**没有**用真实注入器
    /// （调用方自带了注入器，或根本没接输入）。
    pub fn input_stats(&self) -> Option<&std::sync::Arc<beam_input::InjectorStats>> {
        self.input_stats.as_ref()
    }

    /// 音频统计（§3.7）。
    ///
    /// 返回 `None` 表示这次会话**没有**接音频
    /// （`audio_mode=off`，或 `stream_only` 下主机不采集）。
    /// 音频失败**不会**让会话进入错误态：画面照旧，音频降级为关闭。
    pub fn audio_stats(&self) -> Option<&AudioStatsHandle> {
        self.audio_stats.as_ref()
    }

    /// **停止**主机侧音频采集（§3.7）。
    ///
    /// 采集任务被取消 ⇒ WASAPI loopback 设备关闭、Opus 编码器随之释放。
    /// **画面完全不受影响**（音频停止只是降级为「只有画面」，§3.7）。
    ///
    /// 用途有两个：
    ///
    /// - 产品侧：用户按下「静音」；
    /// - 验证侧：`beam-host` 的 `client_loopback` 示例在本机同时跑主机与客户端，
    ///   客户端把收到的音频**回放**到同一个默认渲染端点时，主机的 loopback
    ///   会把这段回放**再采回来** ⇒ 形成反馈回路。
    ///   先停掉采集，客户端再回放，回路就断了。
    ///
    /// 返回 `true` 表示确实停掉了一次采集任务（此前有任务在跑）。
    pub async fn stop_audio(&mut self) -> bool {
        match self.audio_task.take() {
            Some(task) => {
                task.abort();
                // 等任务真正被取消：采集设备随之关闭（`LoopbackInner` 的 `Drop`
                // 会置停止标志并 join 采集线程）。
                let _ = task.await;
                true
            }
            None => false,
        }
    }
}

/// 按 feature 选一条主机侧画面流水线（§3.9）。
///
/// - **`platform` feature 开** → WGC 抓屏（§3.5）+ NVENC 硬编（§3.6）的
///   真后端流水线（[`crate::platform::PlatformPipeline`]）；
/// - **关** → [`crate::adapters::EmptyPipeline`]：控制面、能力协商与
///   配对闸门照常工作，但**一帧画面都不产出**（不伪造）。
///
/// 返回值同时给出流水线的**真实错误槽**：[`crate::runtime::HostSessionHandle::pump`]
/// 会从这里把真实错误冒泡给调用方（§3.9）。
///
/// 为什么需要这个函数：桌面端（`apps/desktop`）按 §4 **不能**直接依赖
/// 平台 crate，只能经由本 crate 拿到流水线；「有没有平台后端」这件事
/// 由本 crate 的 feature 决定，调用方不需要（也不该）知道细节。
pub fn default_frame_pipeline(
    cfg: &HostConfig,
) -> (
    Box<dyn beam_session::FramePipeline>,
    crate::adapters::PipelineErrors,
) {
    #[cfg(any(feature = "platform", feature = "platform-wgc"))]
    {
        let pipeline = crate::platform::PlatformPipeline::new(
            cfg.capture.clone(),
            cfg.encode.clone(),
            (u32::from(cfg.stream.width), u32::from(cfg.stream.height)),
        );
        let errors = pipeline.errors();
        (Box::new(pipeline), errors)
    }
    #[cfg(not(any(feature = "platform", feature = "platform-wgc")))]
    {
        let _ = cfg;
        let pipeline = crate::adapters::EmptyPipeline::new();
        let errors = pipeline.errors();
        (Box::new(pipeline), errors)
    }
}

/// 主机运行时：把平台实现与会话层组装起来。
#[derive(Debug)]
pub struct HostRuntime {
    cfg: HostConfig,
    advertiser: Option<Advertiser>,
    /// 当前会话对端（已配对设备）的权限位（§3.3 信任库 → §3.8 单设备闸门）。
    ///
    /// `None` = 编排层还不知道对端是谁 → 协议侧不施加限制，只受配置侧约束
    /// （见 [`crate::input::map_input_permissions`]）。
    peer_perms: Option<PeerPerms>,
}

impl HostRuntime {
    /// 创建运行时（不启动广播）。
    pub fn new(cfg: HostConfig) -> Self {
        Self {
            cfg,
            advertiser: None,
            peer_perms: None,
        }
    }

    /// 设置当前会话对端（已配对设备）的权限位。
    ///
    /// 这是「**单设备闸门**」的开关：调用方（桌面端）从信任库
    /// （`beam_pair::TrustStore`）里取出对端的 `TrustedPeer.perms` 喂进来，
    /// 主机编排层据此映射出按设备的注入权限
    /// （[`crate::input::map_input_permissions`]）。
    ///
    /// 不调用它 = 单设备闸门**不生效**（只受配置侧全局上限约束）。
    pub fn set_peer_permissions(&mut self, perms: Option<PeerPerms>) {
        self.peer_perms = perms;
    }

    /// 当前会话对端的权限位。
    pub fn peer_permissions(&self) -> Option<PeerPerms> {
        self.peer_perms
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
    ///
    /// ## 为什么没有 `input` 参数
    ///
    /// 输入事件走 §5 的**输入单向流**（客户端 → 主机），必须有一条传输连接
    /// 才能接收；本方法没有连接（纯内存组装），因此不接输入。
    /// 需要输入注入的主机路径请用 [`HostRuntime::start_quic_session`]：
    /// 它会自己起输入泵（§3.8「输入 stream 接收任务 → 有界通道 → 注入线程」）。
    pub fn start_session<C, E, S>(&self, capture: C, encoder: E, sender: S) -> HostSessionHandle
    where
        C: CaptureSource + Send + 'static,
        E: VideoEncoder + Send + 'static,
        S: NetVideoSenderTrait + Send + 'static,
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
            input_stats: None,
            audio_task: None,
            audio_stats: None,
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
        let pipeline = CaptureEncodePipeline::new(capture, encoder, self.cfg.capture.clone());
        // 与 `start_session` 同法：先把共享错误槽取出来，再把流水线移进 `HostDeps`。
        let errors = pipeline.errors();
        self.start_quic_session_with_pipeline(conn, Box::new(pipeline), errors, input)
            .await
    }

    /// 用一条**已经建立**的 QUIC 连接启动主机会话，并注入**已经组装好的**流水线。
    ///
    /// 与 [`HostRuntime::start_quic_session`] 的区别：后者用
    /// `CaptureSource` / `VideoEncoder` 两个平台 trait 现搭流水线，
    /// 因此要求它们满足 `Send`；本方法接受**已经组装好**的
    /// [`beam_session::FramePipeline`]，因此可以用「抓屏与编码在专用 OS 线程上」
    /// 的实现（§6）——那种实现里的编码器**不是 `Send`**（ffmpeg 裸指针）。
    ///
    /// `errors` 是流水线共享的真实错误槽：调用方拿到的
    /// [`HostSessionHandle`] 在 [`HostSessionHandle::pump`] 时会把里面的错误冒泡出来。
    pub async fn start_quic_session_with_pipeline<I>(
        &self,
        conn: beam_net::Conn,
        pipeline: Box<dyn beam_session::FramePipeline>,
        errors: PipelineErrors,
        input: Option<I>,
    ) -> Result<HostSessionHandle, HostError>
    where
        I: PlatformInputSink + Send + 'static,
    {
        let transport = HostTransport::accept(conn.clone()).await.map_err(|e| {
            HostError::TransportUnavailable {
                reason: format!("{e}"),
            }
        })?;

        let deps = HostDeps {
            pipeline,
            sender: Box::new(NetVideoSender::new(conn.clone())),
            // 输入由本 crate 的输入泵驱动（见下），会话层不持有注入器。
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

        // §4.1 时序：QUIC 连接已经建立 → `Connecting` → `Connected`（进入能力协商）。
        //
        // 这一步**不能省**：§3.9 的转移表里 `Idle → Streaming` 是非法转移，
        // 主机必须先走到 `Negotiating`，收到客户端的 `Hello` 才能进 `Streaming`。
        // 不做这一步的话协商会一直失败（`E9001` 非法转移）。
        let _ = session.apply(SessionInput::Connecting);
        let _ = session.apply(SessionInput::Connected);

        // 输入泵：§5 输入 stream（客户端 → 主机）。
        //
        // §3.8 的线程模型：「输入 stream 接收任务 → 有界通道 → 注入线程（高优先级）」。
        // 本任务就是**接收任务**：它只负责把事件投进注入器的有界通道，
        // 真正的 `SendInput` 发生在 `beam-input` 的专用注入线程上。
        //
        // 注入器从哪来：
        //
        // - 调用方给了**占位**注入器（`NoopInputSink`：桌面端受 §4 限制拿不到
        //   平台 crate）→ 主机编排层换成**真实注入器**
        //   （`beam_input::InputInjector`：有界通道 + 专用线程 + MMCSS），
        //   权限 = 配置侧 `InputSettings` × 协议侧对端权限（交集）；
        // - 调用方给了自己的注入器 → 原样使用（测试 / 特殊场景）。
        let mut input_stats: Option<std::sync::Arc<beam_input::InjectorStats>> = None;
        let input_task = input.map(|sink| {
            let conn = conn.clone();
            let mut sink: Box<dyn beam_session::InputSink> = if sink.is_placeholder() {
                let perms = map_input_permissions(self.peer_perms.as_ref(), &self.cfg.input);
                tracing::info!(
                    keyboard = perms.keyboard,
                    mouse = perms.mouse,
                    gamepad = perms.gamepad,
                    "使用真实注入器(有界通道 + 专用注入线程)"
                );
                let injector = InputInjector::new(perms);
                input_stats = Some(injector.stats());
                Box::new(InputAdapter::new(injector))
            } else {
                Box::new(InputAdapter::new(sink))
            };
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

        // 音频（§3.7）：与画面走**同一条** QUIC 连接的 datagram 通道，
        // 只是帧头 `type = 1`（音频）。音频失败**不会**打断画面（降级为只有画面）。
        let (audio_task, audio_stats) = self.start_audio(conn.clone());

        Ok(HostSessionHandle {
            session,
            errors,
            input_task,
            input_stats,
            audio_task,
            audio_stats,
        })
    }

    /// 按 `audio_mode` 启动主机侧音频（§3.7）。
    ///
    /// 返回（任务句柄，统计句柄）：
    ///
    /// - `off` ⇒ 两者都是 `None`（本次会话完全没有音频）；
    /// - `stream_only` ⇒ 任务为 `None`（主机**不采集**系统声音 ⇒ 没有帧可发），
    ///   统计句柄仍然是 `Some`，便于上层看出「音频开了但没帧」；
    /// - `host+stream` ⇒ 打开 WASAPI loopback → Opus 编码 → 发音频数据报。
    ///
    /// 任何一步失败（设备打不开 `E7001`、Opus 后端未启用 `E7004`、编码失败 `E7002`）
    /// 都只影响音频：任务停止、错误码记进统计并写日志，**画面照旧**。
    fn start_audio(
        &self,
        conn: beam_net::Conn,
    ) -> (
        Option<tokio::task::JoinHandle<()>>,
        Option<AudioStatsHandle>,
    ) {
        let cfg = self.cfg.audio.clone();
        match cfg.mode {
            beam_audio::AudioMode::Off => (None, None),
            beam_audio::AudioMode::StreamOnly => {
                // §3.7 `stream_only` = 「只串流（主机不采集）」。
                // 主机侧唯一可用的音频源就是 WASAPI loopback（采系统正在播放的声音），
                // 不采集就没有帧可发 ⇒ 本次会话不发音频帧，画面不受影响。
                tracing::info!("audio_mode=stream_only：主机不采集系统声音,本次会话不发音频帧");
                (None, Some(AudioStatsHandle::default()))
            }
            beam_audio::AudioMode::HostAndStream => {
                let stats = AudioStatsHandle::default();
                // 先建编码器：没开 `opus` feature 时这里是 E7004（后端未启用），
                // 早失败早知道，别把设备打开了才发现编不了。
                let encoder = match beam_audio::OpusEncoderImpl::new(cfg.clone()) {
                    Ok(encoder) => encoder,
                    Err(e) => {
                        stats.record_error(e.code());
                        tracing::warn!(
                            code = %e.code_str(),
                            "Opus 编码器不可用,音频降级为关闭(画面不受影响)"
                        );
                        return (None, Some(stats));
                    }
                };
                let source = beam_audio::WasapiLoopbackSource::new();
                let sender = NetAudioSender::new(conn);
                let task_stats = stats.clone();
                let error_stats = stats.clone();
                let task = tokio::spawn(async move {
                    match crate::audio::run_audio_loop(source, encoder, sender, cfg, task_stats)
                        .await
                    {
                        Ok(()) => {}
                        Err(e) => {
                            error_stats.record_error(e.code());
                            tracing::warn!(
                                code = %e.code_str(),
                                reason = %e,
                                "音频链路失败,降级为只有画面"
                            );
                        }
                    }
                });
                (Some(task), Some(stats))
            }
        }
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
        let handle = runtime.start_session(FakeCapture, FakeEncoder, FakeSender);
        assert_eq!(
            handle.state(),
            beam_session::SessionState::Idle,
            "刚创建时应处于 Idle"
        );
        let _ = FakeInput;
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

    #[test]
    fn config_from_resolved_carries_input_settings() {
        // §3.14 → §3.9：输入设置必须真的送达 HostConfig（主机全局上限）。
        let mut resolved = ResolvedConfig::default();
        resolved.input.allow_keyboard = false;
        resolved.input.allow_mouse = true;
        resolved.input.gamepad = true;

        let cfg = HostConfig::from_resolved(&resolved);
        assert!(!cfg.input.allow_keyboard);
        assert!(cfg.input.allow_mouse);
        assert!(cfg.input.gamepad);
    }

    #[test]
    fn config_from_resolved_carries_audio_settings() {
        // §3.14 → §3.9：音频模式与码率必须真的送达 `HostConfig.audio`。
        let mut resolved = ResolvedConfig::default();
        resolved.audio.mode = beam_config::AudioMode::HostAndStream;
        resolved.audio.bitrate_bps = 96_000;

        let cfg = HostConfig::from_resolved(&resolved);
        assert_eq!(cfg.audio.mode, beam_audio::AudioMode::HostAndStream);
        assert_eq!(cfg.audio.bitrate_bps, 96_000);
        assert_eq!(cfg.audio.sample_rate, 48_000, "§3.7：48 kHz");
        assert_eq!(cfg.audio.channels, 2, "§3.7：双声道");
        assert!(
            cfg.stream.audio_enabled,
            "音频模式不是 off 时,能力协商里必须带上音频"
        );
    }

    #[test]
    fn audio_is_off_by_default() {
        let cfg = HostConfig::default();
        assert_eq!(cfg.audio.mode, beam_audio::AudioMode::Off);
        assert!(!cfg.stream.audio_enabled, "默认不得宣称支持音频");
    }

    #[test]
    fn peer_permissions_are_carried_by_the_runtime() {
        let mut runtime = HostRuntime::new(HostConfig::default());
        assert!(runtime.peer_permissions().is_none(), "默认没有对端权限");
        let perms = PeerPerms {
            view: true,
            input: false,
            audio: true,
            gamepad: true,
        };
        runtime.set_peer_permissions(Some(perms));
        assert_eq!(runtime.peer_permissions(), Some(perms));
    }
}
