//! FFI 导出面（§3.12）。
//!
//! 这一层只做「类型 ↔ Kotlin」的映射与回调桥接，不含业务逻辑。

use beam_discovery::{DiscoveredHost, DiscoveryEvent};
use beam_pair::{Fingerprint, PairPayload, TrustStore};
use beam_proto::StreamConfig;
use beam_session::{PeerAddr, SessionEvent, SessionHandle};

use crate::error::FfiError;

/// 客户端配置（§3.12 `BeamClient::new(config)`）。
#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    /// 设备名（展示用）。
    pub device_name: String,
    /// 平台标识（`android` / `windows`）。
    pub platform: String,
    /// 是否自动重连。
    pub auto_reconnect: bool,
    /// 是否启用日志。
    pub enable_logging: bool,
}

/// 配对后的主机信息（跨 FFI 传递）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// 主机地址。
    pub host: String,
    /// 主机端口。
    pub port: u16,
    /// 主机证书指纹（十六进制）。
    pub fingerprint: String,
}

impl PeerInfo {
    /// 转成会话层使用的地址。
    pub fn to_peer_addr(&self) -> PeerAddr {
        PeerAddr {
            host: self.host.clone(),
            port: self.port,
            fingerprint: Some(self.fingerprint.clone()),
        }
    }

    /// 从会话层地址构造。
    pub fn from_peer_addr(p: &PeerAddr) -> Self {
        Self {
            host: p.host.clone(),
            port: p.port,
            fingerprint: p.fingerprint.clone().unwrap_or_default(),
        }
    }
}

/// 发现事件回调（§3.12 `browse(listener)`）。
pub trait DiscoveryListener: Send {
    /// 发现一个主机。
    fn on_host_found(&mut self, host: &DiscoveredHost);
    /// 主机消失。
    fn on_host_lost(&mut self, fullname: &str);
}

/// 帧 / 事件回调（§3.12 `callback interface FrameSink`）。
///
/// 帧回调在专用线程上触发（§3.12「帧回调在专用线程 → `queue()`」），
/// 调用方必须尽快返回，不要在回调里做重活。
pub trait FrameSink: Send {
    /// 收到一帧画面。
    ///
    /// `data` 在热路径上应当是 JNI DirectByteBuffer 指向的 Rust 侧内存，
    /// 详见 [`crate`] 的模块文档；调用方消费完后需要归还缓冲。
    fn on_frame(&mut self, data: &[u8], pts_us: u64, capture_ts_us: u64, key: bool);

    /// 收到会话事件。
    fn on_event(&mut self, event: SessionEvent);
}

/// 事件桥接：把会话事件转成 `FrameSink` 回调。
#[derive(Debug)]
pub struct EventBridge<S> {
    sink: S,
}

impl<S> EventBridge<S>
where
    S: FrameSink,
{
    /// 组装桥接。
    pub fn new(sink: S) -> Self {
        Self { sink }
    }

    /// 转发一帧。
    pub fn forward_frame(&mut self, data: &[u8], pts_us: u64, capture_ts_us: u64, key: bool) {
        self.sink.on_frame(data, pts_us, capture_ts_us, key);
    }

    /// 转发一个事件。
    pub fn forward_event(&mut self, event: SessionEvent) {
        self.sink.on_event(event);
    }
}

/// Beam 客户端（§3.12 导出面）。
pub struct BeamClient {
    config: ClientConfig,
    trust: TrustStore,
}

impl std::fmt::Debug for BeamClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BeamClient")
            .field("config", &self.config)
            .finish()
    }
}

impl BeamClient {
    /// 创建客户端（§3.12 `BeamClient::new(config)`）。
    pub fn new(config: ClientConfig) -> Result<Self, FfiError> {
        Ok(Self {
            config,
            trust: TrustStore::new(),
        })
    }

    /// 客户端配置。
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// 信任库（配对成功后由主机侧写入，这里只读）。
    pub fn trust_store(&self) -> &TrustStore {
        &self.trust
    }

    /// 开始浏览局域网主机（§3.12 `browse(listener)`）。
    ///
    /// 浏览循环在后台线程里跑，事件通过 `listener` 回调给 Kotlin 侧。
    pub fn browse(&self, listener: Box<dyn DiscoveryListener>) -> Result<(), FfiError> {
        let mut browser =
            beam_discovery::Browser::start().map_err(|_| FfiError::DiscoveryFailed)?;
        std::thread::Builder::new()
            .name("beam-ffi-browse".to_string())
            .spawn(move || {
                use futures::StreamExt;
                let mut listener = listener;
                while let Some(event) = futures::executor::block_on(browser.next()) {
                    match event {
                        DiscoveryEvent::Found(host) => listener.on_host_found(&host),
                        DiscoveryEvent::Lost { fullname } => listener.on_host_lost(&fullname),
                    }
                }
            })
            .map_err(|_| FfiError::DiscoveryFailed)?;
        Ok(())
    }

    /// 扫码配对（§3.12 `pair_qr(payload)`）。
    ///
    /// 载荷格式见 §3.3：`beam://pair?h=ip:port&fp=..&t=token`。
    pub fn pair_qr(&self, payload: &str) -> Result<PeerInfo, FfiError> {
        let parsed = PairPayload::from_uri(payload).map_err(|e| FfiError::InvalidPayload {
            reason: e.to_string(),
        })?;
        Ok(PeerInfo {
            host: parsed.host,
            port: parsed.port,
            fingerprint: parsed.fingerprint.to_hex(),
        })
    }

    /// PIN 配对（§3.12 `pair_pin(peer, pin)`）。
    ///
    /// 这里只做本地校验（PIN 格式 + 指纹格式）；真正的 SPAKE2 交换由会话层
    /// 经 net 传递消息完成（§3.3「PIN 交换消息由 session 经 net 传递」）。
    pub fn pair_pin(&self, peer: &PeerInfo, pin: &str) -> Result<(), FfiError> {
        let policy = beam_pair::PairingPolicy::default();
        beam_pair::validate_pin(pin, &policy).map_err(|_| FfiError::PinFailed)?;
        if !peer.fingerprint.is_empty() {
            Fingerprint::from_hex(&peer.fingerprint).map_err(|e| FfiError::InvalidPayload {
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }

    /// 连接主机（§3.12 `connect(peer, cfg, sink)`）。
    pub fn connect(
        &self,
        peer: PeerInfo,
        cfg: StreamConfig,
        sink: Box<dyn FrameSink>,
    ) -> Result<SessionHandle, FfiError> {
        let _ = (cfg, sink);
        let handle = beam_session::ClientSession::connect(
            peer.to_peer_addr(),
            beam_session::ClientSessionConfig::default(),
        );
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopSink;
    impl FrameSink for NoopSink {
        fn on_frame(&mut self, _data: &[u8], _pts_us: u64, _capture_ts_us: u64, _key: bool) {}
        fn on_event(&mut self, _event: SessionEvent) {}
    }

    #[test]
    fn new_client_works() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        assert_eq!(client.config().platform, "");
    }

    #[test]
    fn pair_qr_parses_payload() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let fp = Fingerprint::from_bytes([0xAB; 32]);
        let payload = PairPayload {
            host: "192.168.1.10".into(),
            port: 4827,
            fingerprint: fp,
            token: "tok".into(),
        };
        let peer = client.pair_qr(&payload.to_uri()).expect("应当能解析");
        assert_eq!(peer.host, "192.168.1.10");
        assert_eq!(peer.port, 4827);
        assert_eq!(peer.fingerprint, fp.to_hex());
    }

    #[test]
    fn pair_qr_rejects_garbage() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let err = client.pair_qr("not a uri").expect_err("必须失败");
        assert_eq!(err.code(), 14001);
    }

    #[test]
    fn pair_pin_validates_length() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let peer = PeerInfo {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: String::new(),
        };
        let err = client.pair_pin(&peer, "1").expect_err("PIN 太短必须失败");
        assert_eq!(err.code(), 14002);
    }

    #[test]
    fn connect_returns_handle() {
        let client = BeamClient::new(ClientConfig::default()).expect("应当能创建");
        let peer = PeerInfo {
            host: "127.0.0.1".into(),
            port: 4827,
            fingerprint: String::new(),
        };
        let handle = client
            .connect(peer, StreamConfig::default(), Box::new(NoopSink))
            .expect("应当能拿到句柄");
        assert_eq!(handle.state(), beam_session::SessionState::Idle);
    }

    #[test]
    fn event_bridge_forwards() {
        let mut bridge = EventBridge::new(NoopSink);
        bridge.forward_frame(&[1, 2, 3], 1, 2, true);
        bridge.forward_event(SessionEvent::Closed);
    }
}
