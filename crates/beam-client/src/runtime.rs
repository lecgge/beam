//! 客户端运行时组装（§3.9 / §3.10）。

use std::sync::Arc;

use beam_config::ResolvedConfig;
use beam_decode::{D3D11VaDecoder, VideoDecoder};
use beam_discovery::{Browser, DiscoveryEvent};
use beam_net::PeerVerifier;
use beam_proto::StreamConfig;
use beam_render::{D3D11Renderer, Renderer};
use beam_session::{
    ClientControlDeps, ClientDeps, ClientSession, ClientSessionConfig, PairingConfig, PeerAddr,
    SessionHandle, SessionTimeouts, TransportFactory,
};

use crate::adapters::DecodeRenderConsumer;

/// 会话 + 它的原生串流窗口（§3.10 / 产品方案第 45 行）。
///
/// ## 字段顺序有讲究
///
/// **窗口必须比会话活得久**：D3D11 交换链挂在窗口（`HWND`）上，
/// 窗口先被销毁、交换链后被销毁，DXGI 就会拿着一个已死的 `HWND` 收尾。
/// Rust 的结构体字段按**声明顺序** drop，因此窗口字段放在**后面** ——
/// 这样会话先走、窗口后走。
///
/// 上层只要把整个 [`WindowedSession`] 持有到会话结束即可，不必操心顺序。
#[derive(Debug)]
pub struct WindowedSession {
    /// 会话句柄。
    pub handle: SessionHandle,
    /// 原生串流窗口（**必须**活到会话结束）。
    pub window: crate::VideoWindow,
}

/// 客户端编排层错误。
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// 发现服务不可用（`E4xxx`）。
    #[error("发现服务不可用: {reason}")]
    DiscoveryUnavailable {
        /// 原因。
        reason: String,
    },
    /// 上屏后端不可用（§3.10）。
    ///
    /// 典型原因：`video` feature 没开（`beam-render` 的 `d3d11` 后端没编进来）、
    /// 交换链创建失败、或原生窗口句柄无效。
    ///
    /// **如实报错，不静默降级** —— 上层必须知道「画面没上屏」。
    #[error("上屏后端不可用: {reason}")]
    PresentationUnavailable {
        /// 原因。
        reason: String,
    },
}

/// 客户端配置。
#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    /// 串流配置。
    pub stream: StreamConfig,
    /// 要连接的主机（为空表示由调用方在连接时指定）。
    pub peer: Option<PeerAddr>,
    /// 音频配置（§3.7）。
    ///
    /// `mode = off`（默认）时客户端**不接音频**；非 off 时用
    /// [`ClientRuntime::transport_factory_with_audio`] 建立带音频分流的传输。
    pub audio: beam_audio::AudioConfig,
    /// 客户端输入的 PIN（§3.3，4 位数字）。
    ///
    /// 有值时客户端在控制通道上发起 SPAKE2 配对（§4.1「`PairRequest`
    /// (token 或 SPAKE2 msg)」），**配对成功之后才发 `Hello`**。
    ///
    /// `None` = 本次连接不需要配对（对端已在信任库里，§3.3
    /// 「已配对的主机可传空串」）。
    pub pin: Option<String>,
    /// **本机**证书指纹（放进挑战载荷，让主机知道客户端是谁，§3.3）。
    pub fingerprint: Option<beam_pair::Fingerprint>,
}

impl ClientConfig {
    /// 从解析后的配置生成（§3.14）。
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
            peer: None,
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
            // §3.3：默认**不配对**（对端已在信任库里的情形）。
            // 需要配对时由调用方设置 `pin` / `fingerprint`。
            pin: None,
            fingerprint: None,
        }
    }
}

/// 客户端运行时。
pub struct ClientRuntime {
    cfg: ClientConfig,
    /// 客户端输入采集源（§3.8）：有它才回传键鼠。
    input: Option<Arc<dyn beam_session::InputSource>>,
}

impl std::fmt::Debug for ClientRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientRuntime")
            .field("cfg", &self.cfg)
            .field("has_input", &self.input.is_some())
            .finish()
    }
}

impl ClientRuntime {
    /// 创建运行时。
    pub fn new(cfg: ClientConfig) -> Self {
        Self { cfg, input: None }
    }

    /// 装上**客户端输入采集源**（§3.8）。
    ///
    /// 装上之后，本次会话在 `Streaming` 期间会把采集到的键鼠事件
    /// 写进 §5 的**输入单向流**（客户端 → 主机），主机侧再注入到自己的桌面。
    ///
    /// 真机实现：[`crate::capture::WindowsInputCapture`]（低级键鼠钩子）。
    pub fn with_input(mut self, input: Arc<dyn beam_session::InputSource>) -> Self {
        self.input = Some(input);
        self
    }

    /// 当前配置。
    pub fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    /// 开始浏览局域网主机（§3.4）。
    pub fn browse(&self) -> Result<Browser, ClientError> {
        Browser::start().map_err(|e| ClientError::DiscoveryUnavailable {
            reason: format!("{}: {e}", e.code_str()),
        })
    }

    /// 读取下一个发现事件。
    pub fn next_discovery(browser: &mut Browser) -> Option<DiscoveryEvent> {
        use futures::StreamExt;
        futures::executor::block_on(browser.next())
    }

    /// 组装并启动客户端会话（§3.9 `ClientSession::connect(peer, cfg)`）。
    pub fn connect<D, R>(
        &self,
        peer: PeerAddr,
        decoder: D,
        renderer: R,
        transport: Option<Box<dyn TransportFactory>>,
    ) -> SessionHandle
    where
        D: VideoDecoder + Send + 'static,
        R: Renderer + Send + 'static,
    {
        let consumer = DecodeRenderConsumer::new(decoder, renderer);
        let deps = ClientDeps {
            consumer: Some(Box::new(consumer)),
            transport,
            // §3.8：客户端输入采集源（没有它就不回传输入）。
            input: self.input.clone(),
        };
        let session_cfg = ClientSessionConfig {
            stream: self.cfg.stream.clone(),
            reconnect_timeout: SessionTimeouts::default().reconnect_timeout,
            stall_threshold: SessionTimeouts::default().stall_threshold,
            transport: None,
        };
        // §3.3：把客户端输入的 PIN 与本机指纹交给控制层 ——
        // 有 PIN 就发起 SPAKE2 配对，**配对成功之后才发 `Hello`**。
        let ctrl_deps = ClientControlDeps {
            pairing: PairingConfig {
                pin: self.cfg.pin.clone(),
                fingerprint: self.cfg.fingerprint,
                ..PairingConfig::default()
            },
            rate: None,
        };
        ClientSession::connect_with_control(peer, session_cfg, deps, ctrl_deps)
    }

    /// 用**默认的解码 + 上屏组合**建立客户端会话（§3.9 / §3.10）。
    ///
    /// §4 规定平台 crate（decode / render）只能被 host / client 引用，
    /// 因此桌面端（§4 的 L4 外壳）不应直接依赖 `beam-decode` / `beam-render`；
    /// 默认组合由本 crate 提供，桌面端只调用这一个入口。
    ///
    /// 平台后端（`d3d11` feature）未启用时，解码会返回 `E10004`
    /// （`beam_decode::error::code::BACKEND_DISABLED`），
    /// 错误原样透传到会话事件，不会被伪装成成功。
    pub fn connect_default(
        &self,
        peer: PeerAddr,
        transport: Option<Box<dyn TransportFactory>>,
    ) -> SessionHandle {
        self.connect(peer, D3D11VaDecoder::new(), D3D11Renderer::new(), transport)
    }

    /// 建立客户端会话，并把解码结果上屏到**原生窗口**（§3.10 / 产品方案第 45 行）。
    ///
    /// ## 为什么这个入口在 L3（`beam-client`）而不在桌面端
    ///
    /// §4 规定平台 crate（decode / render）**只能**被 host / client 引用，
    /// 桌面端（L4）不得直接依赖 `beam-decode` / `beam-render`。
    /// 因此「解码 + 上屏」这条组装线必须住在本 crate，
    /// 上层只提供**一个原生窗口句柄**。
    ///
    /// ## 参数
    ///
    /// - `title`：原生窗口的标题。
    /// - `width` / `height`：窗口客户区尺寸（像素）。
    /// - `transport`：可选的传输工厂（见 [`ClientRuntime::connect`]）。
    ///
    /// ## 返回值
    ///
    /// 返回 `(会话句柄, 窗口)`：**窗口必须活到会话结束** ——
    /// 它是交换链的宿主，提前销毁会让上屏失败。
    /// 调用方把它存着即可（`Drop` 时自动关闭）。
    ///
    /// ## 失败怎么办
    ///
    /// 失败一律返回 [`ClientError::PresentationUnavailable`]，**如实报错**：
    /// 窗口建不出来、交换链建不出来、解码后端没编进来，都不静默降级
    /// （静默降级会让上层以为「在串流」，实际上一帧都没上屏）。
    ///
    /// ## feature
    ///
    /// 真实上屏需要 `video` feature（它同时打开 `beam-render/d3d11`、
    /// `beam-decode/ffmpeg` 与 `beam-window/windows-api`）。
    /// 没开时本方法**照常存在**，但如实报错。
    #[cfg(feature = "video")]
    pub fn connect_to_window(
        &self,
        peer: PeerAddr,
        title: &str,
        width: u32,
        height: u32,
        transport: Option<Box<dyn TransportFactory>>,
    ) -> Result<WindowedSession, ClientError> {
        // 先建原生窗口（§3.10 / 产品方案第 45 行：不走 WebView）。
        let window = crate::VideoWindow::create(title, width, height).map_err(|e| {
            ClientError::PresentationUnavailable {
                reason: format!("创建原生串流窗口失败: {e}"),
            }
        })?;

        let mut renderer = D3D11Renderer::new();
        let render_cfg = beam_render::RenderConfig {
            width,
            height,
            refresh_rate: self.cfg.stream.fps,
            ..beam_render::RenderConfig::default()
        };
        renderer
            .attach(beam_render::WindowHandle(window.hwnd()), render_cfg)
            .map_err(|e| ClientError::PresentationUnavailable {
                reason: format!("交换链创建失败 {}: {e}", e.code_str()),
            })?;

        // §3.10「解码纹理直接交给 D3D11 flip-model 交换链」：
        // D3D11 **不允许跨设备拷贝**，因此解码器必须复用渲染器的**同一个设备**。
        let device = renderer
            .device()
            .ok_or_else(|| ClientError::PresentationUnavailable {
                reason: "渲染器没有 D3D11 设备（beam-render 的 d3d11 后端未编译进来）".to_string(),
            })?;
        let decode_cfg = beam_decode::DecodeConfig {
            codec: self.cfg.stream.codec,
            ..beam_decode::DecodeConfig::default()
        };
        let decoder = D3D11VaDecoder::open_with_device(decode_cfg, device).map_err(|e| {
            ClientError::PresentationUnavailable {
                reason: format!("硬解后端不可用 {}: {e}", e.code_str()),
            }
        })?;

        Ok(WindowedSession {
            handle: self.connect(peer, decoder, renderer, transport),
            window,
        })
    }

    /// 建立客户端会话，并把解码结果上屏到**原生窗口**（§3.10 / 产品方案第 45 行）。
    ///
    /// 没开 `video` feature 时的版本：**如实报错**，不静默降级。
    ///
    /// 见开 `video` feature 的版本（同名方法）了解完整语义。
    #[cfg(not(feature = "video"))]
    pub fn connect_to_window(
        &self,
        _peer: PeerAddr,
        _title: &str,
        _width: u32,
        _height: u32,
        _transport: Option<Box<dyn TransportFactory>>,
    ) -> Result<WindowedSession, ClientError> {
        Err(ClientError::PresentationUnavailable {
            reason: "当前构建未启用 `video` feature（beam-render/d3d11 + beam-decode/ffmpeg + beam-window/windows-api），无法上屏"
                .to_string(),
        })
    }

    /// 用给定的校验器建立传输工厂。
    pub fn transport_factory(
        peer: &PeerAddr,
        net_cfg: beam_net::NetConfig,
        verifier: std::sync::Arc<dyn PeerVerifier>,
    ) -> crate::adapters::NetTransportFactory {
        let addr = format!("{}:{}", peer.host, peer.port)
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:0".parse().expect("合法回环地址"));
        crate::adapters::NetTransportFactory::new(addr, net_cfg, verifier)
    }

    /// 用给定的校验器建立**带音频分流**的传输工厂（§3.7）。
    ///
    /// 与 [`ClientRuntime::transport_factory`] 的区别：datagram 通道会按帧头
    /// `type` 分流 —— 音频数据报走「Opus 解码 + WASAPI 输出」，
    /// 画面数据报走画面重组器（见 [`crate::transport::DemuxTransport`]）。
    ///
    /// 音频不可用（没有输出设备、Opus 后端未启用）时**只关音频**，画面照旧。
    pub fn transport_factory_with_audio(
        peer: &PeerAddr,
        net_cfg: beam_net::NetConfig,
        verifier: std::sync::Arc<dyn PeerVerifier>,
        audio: beam_audio::AudioConfig,
    ) -> crate::transport::DemuxTransportFactory {
        let addr = format!("{}:{}", peer.host, peer.port)
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:0".parse().expect("合法回环地址"));
        crate::transport::DemuxTransportFactory::with_audio(addr, net_cfg, verifier, audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam_decode::{D3D11VaDecoder, DecError};
    use beam_render::{D3D11Renderer, RenderError};

    struct FakeDecoder;
    impl VideoDecoder for FakeDecoder {
        fn decode(
            &mut self,
            f: beam_proto::AssembledFrame,
        ) -> Result<beam_proto::DecodedFrame, DecError> {
            Ok(beam_proto::DecodedFrame {
                frame_id: f.frame_id,
                pts_us: f.pts_us,
                capture_ts_us: f.capture_ts_us,
                width: 1920,
                height: 1080,
                is_key: f.is_key,
                surface: beam_proto::PlatformHandle::NONE,
            })
        }
        fn reset(&mut self) -> Result<(), DecError> {
            Ok(())
        }
    }

    struct FakeRenderer;
    impl Renderer for FakeRenderer {
        fn present(&mut self, _f: &beam_proto::DecodedFrame) -> Result<(), RenderError> {
            Ok(())
        }
    }

    /// 测试用校验器：信任一切指纹。
    #[derive(Debug)]
    struct AllowAllVerifier;
    impl PeerVerifier for AllowAllVerifier {
        fn verify(&self, _fp: &beam_pair::Fingerprint) -> bool {
            true
        }
    }

    #[test]
    fn runtime_builds_peer_address() {
        let peer = PeerAddr {
            host: "192.168.1.10".into(),
            port: 4827,
            fingerprint: None,
        };
        let runtime = ClientRuntime::new(ClientConfig::default());
        let factory = ClientRuntime::transport_factory(
            &peer,
            beam_net::NetConfig::default(),
            std::sync::Arc::new(AllowAllVerifier),
        );
        let _ = factory;
        assert_eq!(runtime.config().stream.fps, 60);
    }

    #[test]
    fn connect_without_transport_stays_idle() {
        let runtime = ClientRuntime::new(ClientConfig::default());
        let peer = PeerAddr {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: None,
        };
        let handle = runtime.connect(peer, FakeDecoder, FakeRenderer, None);
        assert_eq!(handle.state(), beam_session::SessionState::Idle);
        let _ = (D3D11VaDecoder::new(), D3D11Renderer::new());
    }
}
