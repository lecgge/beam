//! 客户端运行时组装（§3.9 / §3.10）。

use beam_config::ResolvedConfig;
use beam_decode::{D3D11VaDecoder, VideoDecoder};
use beam_discovery::{Browser, DiscoveryEvent};
use beam_net::PeerVerifier;
use beam_proto::StreamConfig;
use beam_render::{D3D11Renderer, Renderer};
use beam_session::{
    ClientDeps, ClientSession, ClientSessionConfig, PeerAddr, SessionHandle, SessionTimeouts,
    TransportFactory,
};

use crate::adapters::DecodeRenderConsumer;

/// 客户端编排层错误。
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// 发现服务不可用（`E4xxx`）。
    #[error("发现服务不可用: {reason}")]
    DiscoveryUnavailable {
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
        }
    }
}

/// 客户端运行时。
#[derive(Debug)]
pub struct ClientRuntime {
    cfg: ClientConfig,
}

impl ClientRuntime {
    /// 创建运行时。
    pub fn new(cfg: ClientConfig) -> Self {
        Self { cfg }
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
        };
        let session_cfg = ClientSessionConfig {
            stream: self.cfg.stream.clone(),
            reconnect_timeout: SessionTimeouts::default().reconnect_timeout,
            stall_threshold: SessionTimeouts::default().stall_threshold,
            transport: None,
        };
        ClientSession::connect_with(peer, session_cfg, deps)
    }

    /// 用**默认的解码 + 上屏组合**建立客户端会话（§3.9 / §3.10）。
    ///
    /// §4 规定平台 crate（decode / render）只能被 host / client 引用，
    /// 因此桌面端（§4 的 L4 外壳）不应直接依赖 `beam-decode` / `beam-render`；
    /// 默认组合由本 crate 提供，桌面端只调用这一个入口。
    ///
    /// 平台后端（`d3d11` feature）未启用时，解码会返回 `E10005`，
    /// 错误原样透传到会话事件，不会被伪装成成功。
    pub fn connect_default(
        &self,
        peer: PeerAddr,
        transport: Option<Box<dyn TransportFactory>>,
    ) -> SessionHandle {
        self.connect(peer, D3D11VaDecoder::new(), D3D11Renderer::new(), transport)
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
