//! uniffi 导出面（§3.12）。
//!
//! 本模块**只在 `uniffi` feature 打开时编译**（`cargo check -p beam-ffi` 默认不编译），
//! 对外提供 Kotlin 侧可见的类型与方法：
//!
//! | Kotlin 生成物 | Rust 类型 | 说明 |
//! |---|---|---|
//! | `class BeamClient` | [`BeamClient`] | §3.12 点名的五个方法 |
//! | `interface FrameSink` | [`FrameSink`] | 帧 / 事件回调（callback interface） |
//! | `interface DiscoveryListener` | [`DiscoveryListener`] | 发现回调（callback interface） |
//! | `data class Peer` | [`Peer`] | 主机信息 |
//! | `data class StreamCfg` | [`StreamCfg`] | 串流配置 |
//! | `data class BeamClientConfig` | [`BeamClientConfig`] | 客户端配置 |
//! | `data class DiscoveredHost` | [`DiscoveredHost`] | 发现结果 |
//! | `data class SessionStats` | [`SessionStats`] | 统计上报 |
//! | `sealed class SessionEvent` | [`SessionEvent`] | 控制面事件 |
//! | `enum SessionState` | [`SessionState`] | 会话状态（§3.9） |
//! | `class Session` | [`Session`] | 会话句柄 |
//!
//! ## 与 Rust 内部类型的映射
//!
//! 内部类型（`beam_proto::StreamConfig`、`beam_session::SessionEvent` 等）不属于本 crate，
//! 无法直接 `#[derive(uniffi::…)]`（孤儿规则），因此这里定义**镜像类型**并在导出方法里转换：
//!
//! | 导出面类型 | 内部类型 | 转换 |
//! |---|---|---|
//! | [`BeamClientConfig`] | [`crate::client::ClientConfig`] | [`BeamClientConfig::to_core`] |
//! | [`Peer`] | [`crate::client::PeerInfo`] | [`Peer::to_core`] / [`Peer::from_core`] |
//! | [`StreamCfg`] | `beam_proto::StreamConfig` | [`StreamCfg::to_core`] / [`StreamCfg::from_core`] |
//! | [`DiscoveredHost`] | `beam_discovery::DiscoveredHost` | [`DiscoveredHost::from_core`] |
//! | [`SessionEvent`] | `beam_session::SessionEvent` | [`SessionEvent::from_core`] |
//! | [`SessionState`] | `beam_session::SessionState` | [`SessionState::from_core`] |
//!
//! ## 已知缺口（本批未实现，勿当成已完成）
//!
//! 1. **热路径 JNI DirectByteBuffer 未实现**：帧数据经 uniffi 的 `Vec<u8>` 传递，
//!    每帧一次拷贝（[`FrameSink::on_frame`]）。设计见 crate 级文档「热路径」一节。
//! 2. **`connect` 仍然丢弃 `cfg` 与 `sink`**：Rust 侧
//!    `crates/beam-ffi/src/client.rs` 的 `connect` 仍是 `let _ = (cfg, sink);` 占位，
//!    会话停留在 `Idle`，[`FrameSink`] 不会收到任何帧或事件。
//! 3. **帧回调没有专用线程**：[`FrameSink::on_frame`] 目前由调用方线程同步触发。

use std::sync::Arc;

use beam_discovery::DiscoveredHost as CoreDiscoveredHost;
use beam_proto::{CodecKind, StatsReport, StreamConfig as CoreStreamConfig};
use beam_session::{
    SessionEvent as CoreSessionEvent, SessionHandle, SessionState as CoreSessionState,
};

use crate::client::{
    BeamClient as CoreClient, ClientConfig as CoreClientConfig,
    DiscoveryListener as CoreDiscoveryListener, FrameSink as CoreFrameSink,
    PeerInfo as CorePeerInfo,
};
use crate::error::{message as error_text, FfiError as CoreFfiError};

/// uniffi 契约版本号（与根清单里的 `uniffi = "0.29"` 对应）。
///
/// 由 `build.rs` 注入，Android 构建脚本可用它校验「生成的 Kotlin 绑定」
/// 与「`libbeam_ffi.so`」来自同一份契约。
pub const UNIFFI_CONTRACT_VERSION: &str = env!("BEAM_FFI_UNIFFI_CONTRACT_VERSION");

/// 导出面错误（Kotlin 侧表现为异常）。
///
/// 与 Rust 内部的 [`crate::error::FfiError`] 一一对应，保留 `E14xxx` 错误码语义（§6）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BeamError {
    /// `E14001`：配对载荷非法。
    #[error("E14001: 配对二维码内容无效: {reason}")]
    InvalidPayload {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E14002`：PIN 配对失败。
    #[error("E14002: PIN 码校验失败")]
    PinFailed,

    /// `E14003`：连接失败。
    #[error("E14003: 连接主机失败: {reason}")]
    ConnectFailed {
        /// 原因，仅用于日志。
        reason: String,
    },

    /// `E14004`：发现服务不可用。
    #[error("E14004: 无法发现局域网主机")]
    DiscoveryFailed,

    /// `E14005`：功能未启用或参数不受支持。
    #[error("E14005: 当前平台不支持该功能")]
    Unsupported,
}

impl From<CoreFfiError> for BeamError {
    fn from(err: CoreFfiError) -> Self {
        match err {
            CoreFfiError::InvalidPayload { reason } => Self::InvalidPayload { reason },
            CoreFfiError::PinFailed => Self::PinFailed,
            CoreFfiError::ConnectFailed { reason } => Self::ConnectFailed { reason },
            CoreFfiError::DiscoveryFailed => Self::DiscoveryFailed,
            CoreFfiError::Unsupported => Self::Unsupported,
        }
    }
}

/// 客户端配置（§3.12 `BeamClient::new(config)`）。
///
/// 字段与 Android `core-net` 的 `BeamClientConfig` 对齐（见
/// `android/core-net/README.md` §4「uniffi 导出面约定」）。
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BeamClientConfig {
    /// 客户端显示名（对应内部配置的 `device_name`）。
    pub client_name: String,
    /// 日志级别（空串或 `off` 表示不启用日志）。
    pub log_level: String,
    /// FEC 冗余比例（0.0–1.0）。
    ///
    /// 内部 [`crate::client::ClientConfig`] 暂无该字段，当前不参与会话配置。
    pub fec_ratio: f32,
    /// 抖动缓冲帧数。
    ///
    /// 内部 [`crate::client::ClientConfig`] 暂无该字段，当前不参与会话配置。
    pub jitter_frames: i32,
    /// 连接超时（毫秒）。
    ///
    /// 内部 [`crate::client::ClientConfig`] 暂无该字段，当前不参与会话配置。
    pub connect_timeout_ms: i32,
    /// 重连超时（毫秒）；大于 0 表示启用自动重连。
    ///
    /// 内部 [`crate::client::ClientConfig`] 只有 `auto_reconnect` 开关，
    /// 这里用「超时是否配置」推导该开关。
    pub reconnect_timeout_ms: i32,
}

impl BeamClientConfig {
    /// 转成内部客户端配置。
    pub fn to_core(&self) -> CoreClientConfig {
        CoreClientConfig {
            device_name: self.client_name.clone(),
            platform: "android".to_string(),
            auto_reconnect: self.reconnect_timeout_ms > 0,
            enable_logging: !self.log_level.is_empty() && self.log_level != "off",
        }
    }
}

/// 主机信息（§3.12 `peer`）。
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Peer {
    /// 设备 id。
    ///
    /// 内部 [`crate::client::PeerInfo`] 没有设备 id 字段，当前恒为空串。
    pub id: String,
    /// 设备名。
    ///
    /// 内部 [`crate::client::PeerInfo`] 没有设备名字段，当前恒为空串。
    pub name: String,
    /// 主机地址，格式 `host:port`。
    pub address: String,
    /// 主机证书指纹（十六进制）。
    pub fingerprint: String,
    /// 平台标识。
    ///
    /// 内部 [`crate::client::PeerInfo`] 没有平台字段，当前恒为空串。
    pub platform: String,
    /// 是否已在信任库中。
    ///
    /// Rust 侧的 `pair_pin` 目前不查信任库，导出面恒为 `false`。
    pub trusted: bool,
}

impl Peer {
    /// 由内部主机信息构造（地址拼成 `host:port`）。
    pub fn from_core(peer: &CorePeerInfo) -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: format!("{}:{}", peer.host, peer.port),
            fingerprint: peer.fingerprint.clone(),
            platform: String::new(),
            trusted: false,
        }
    }

    /// 转成内部主机信息；地址格式非法时返回 [`BeamError::InvalidPayload`]。
    pub fn to_core(&self) -> Result<CorePeerInfo, BeamError> {
        let (host, port) =
            split_address(&self.address).ok_or_else(|| BeamError::InvalidPayload {
                reason: format!("地址格式非法: {}", self.address),
            })?;
        Ok(CorePeerInfo {
            host,
            port,
            fingerprint: self.fingerprint.clone(),
        })
    }
}

/// 串流配置（§3.12 `cfg`）。
///
/// 数值类型刻意用**有符号**整数与 `f32`：Kotlin 侧对应 `Int` / `Float`，
/// 与 Android `core-net` 的 `StreamCfg` 模型逐字段同型（反射装配按参数名 + 类型匹配）。
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StreamCfg {
    /// 画面宽（像素）。
    pub width: i32,
    /// 画面高（像素）。
    pub height: i32,
    /// 目标帧率。
    pub fps: i32,
    /// 编解码器名：`h264` / `hevc` / `av1`（大小写不敏感）。
    pub codec: String,
    /// 目标码率（kbit/s）。
    pub bitrate_kbps: i32,
    /// FEC 冗余比例（0.0–1.0）。
    ///
    /// `beam_proto::StreamConfig` 暂无该字段，转成内部配置时被丢弃。
    pub fec_ratio: f32,
    /// 是否启用音频。
    pub audio_enabled: bool,
}

impl StreamCfg {
    /// 转成内部串流配置；编解码器名不认识或分辨率越界时返回错误。
    pub fn to_core(&self) -> Result<CoreStreamConfig, BeamError> {
        let width = u16::try_from(self.width)
            .ok()
            .filter(|w| *w > 0)
            .ok_or_else(|| BeamError::InvalidPayload {
                reason: format!("画面宽非法: {}", self.width),
            })?;
        let height = u16::try_from(self.height)
            .ok()
            .filter(|h| *h > 0)
            .ok_or_else(|| BeamError::InvalidPayload {
                reason: format!("画面高非法: {}", self.height),
            })?;
        let fps = u32::try_from(self.fps)
            .ok()
            .filter(|f| *f > 0)
            .ok_or_else(|| BeamError::InvalidPayload {
                reason: format!("帧率非法: {}", self.fps),
            })?;
        Ok(CoreStreamConfig {
            width,
            height,
            fps,
            bitrate_bps: self.bitrate_kbps.max(0) as u32 * 1000,
            codec: parse_codec(&self.codec)?,
            audio_enabled: self.audio_enabled,
            features: 0,
        })
    }

    /// 由内部串流配置构造。
    pub fn from_core(cfg: &CoreStreamConfig) -> Self {
        Self {
            width: cfg.width as i32,
            height: cfg.height as i32,
            fps: cfg.fps as i32,
            codec: cfg.codec.as_str().to_string(),
            bitrate_kbps: (cfg.bitrate_bps / 1000) as i32,
            fec_ratio: 0.0,
            audio_enabled: cfg.audio_enabled,
        }
    }
}

/// 发现到的主机（[`DiscoveryListener::on_host`] 的载荷）。
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DiscoveredHost {
    /// 设备 id。
    pub id: String,
    /// 设备名。
    pub name: String,
    /// 版本串。
    pub version: String,
    /// 指纹前 8 位。
    pub fingerprint: String,
    /// 服务端口。
    pub port: u16,
    /// 地址。
    pub address: String,
    /// mDNS 全名。
    pub fullname: String,
}

impl DiscoveredHost {
    /// 由内部发现记录构造。
    pub fn from_core(host: &CoreDiscoveredHost) -> Self {
        Self {
            id: host.id.clone(),
            name: host.name.clone(),
            version: host.version.clone(),
            fingerprint: host.fingerprint.clone(),
            port: host.port,
            address: host.address.to_string(),
            fullname: host.fullname.clone(),
        }
    }
}

/// 会话状态（§3.9 状态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SessionState {
    /// 空闲（初始态）。
    Idle,
    /// 正在发现主机。
    Discovering,
    /// 正在配对。
    Pairing,
    /// 正在连接。
    Connecting,
    /// 正在协商能力。
    Negotiating,
    /// 正在串流。
    Streaming,
    /// 断线重连中。
    Reconnecting,
    /// 已关闭（终态）。
    Closed,
    /// 错误（可转入 `Closed`）。
    Error,
}

impl SessionState {
    /// 由内部会话状态构造。
    pub fn from_core(state: CoreSessionState) -> Self {
        match state {
            CoreSessionState::Idle => Self::Idle,
            CoreSessionState::Discovering => Self::Discovering,
            CoreSessionState::Pairing => Self::Pairing,
            CoreSessionState::Connecting => Self::Connecting,
            CoreSessionState::Negotiating => Self::Negotiating,
            CoreSessionState::Streaming => Self::Streaming,
            CoreSessionState::Reconnecting => Self::Reconnecting,
            CoreSessionState::Closed => Self::Closed,
            CoreSessionState::Error => Self::Error,
        }
    }
}

/// 统计上报（§3.11 `session://stats`）。
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SessionStats {
    /// 会话号。
    pub session_id: u64,
    /// 实际接收帧率。
    pub fps: u32,
    /// 实测接收码率（bit/s）。
    pub bitrate_bps: u32,
    /// 丢包率（百万分比）。
    pub loss_ppm: u32,
    /// 往返时延（微秒）。
    pub rtt_us: u32,
    /// 解码耗时（微秒）。
    pub decode_us: u32,
    /// 上屏等待（微秒）。
    pub render_us: u32,
    /// 端到端延迟（微秒）。
    pub e2e_us: u32,
    /// 累计掉帧数。
    pub dropped_frames: u32,
}

impl SessionStats {
    /// 由内部统计上报构造。
    pub fn from_core(stats: &StatsReport) -> Self {
        Self {
            session_id: stats.session_id,
            fps: stats.fps,
            bitrate_bps: stats.bitrate_bps,
            loss_ppm: stats.loss_ppm,
            rtt_us: stats.rtt_us,
            decode_us: stats.decode_us,
            render_us: stats.render_us,
            e2e_us: stats.e2e_us,
            dropped_frames: stats.dropped_frames,
        }
    }
}

/// 会话事件（§6 错误传播 / §3.11 事件流）。
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SessionEvent {
    /// 状态变化。
    StateChanged {
        /// 新状态。
        state: SessionState,
    },
    /// 配对完成。
    Paired {
        /// 对端指纹（十六进制）。
        fingerprint: String,
    },
    /// 能力协商完成。
    Negotiated {
        /// 最终串流配置。
        config: StreamCfg,
    },
    /// 统计上报。
    Stats {
        /// 统计快照。
        stats: SessionStats,
    },
    /// 出错（§6：UI 只根据 code 显示文案）。
    Error {
        /// 错误码（`E` 前缀，如 `E9002`）。
        code: String,
        /// 人类可读文案。
        message: String,
        /// 是否值得重试。
        retryable: bool,
    },
    /// 会话结束。
    Closed,
}

impl SessionEvent {
    /// 由内部会话事件构造。
    pub fn from_core(event: CoreSessionEvent) -> Self {
        match event {
            CoreSessionEvent::StateChanged { state } => Self::StateChanged {
                state: SessionState::from_core(state),
            },
            CoreSessionEvent::Paired { fingerprint } => Self::Paired { fingerprint },
            CoreSessionEvent::Negotiated { config } => Self::Negotiated {
                config: StreamCfg::from_core(&config),
            },
            CoreSessionEvent::Stats(stats) => Self::Stats {
                stats: SessionStats::from_core(&stats),
            },
            CoreSessionEvent::Error { code, retryable } => Self::Error {
                code: format!("E{code}"),
                message: error_text(code).to_string(),
                retryable,
            },
            CoreSessionEvent::Closed => Self::Closed,
        }
    }
}

/// 帧 / 事件回调（§3.12 `callback interface FrameSink`）。
///
/// **当前是拷贝路径**：`data` 由 uniffi 的 `Vec<u8>` 传递，每帧一次拷贝；
/// 热路径的 JNI DirectByteBuffer 直传尚未实现（见 crate 级文档「热路径」一节）。
#[uniffi::export(callback_interface)]
pub trait FrameSink: Send {
    /// 收到一帧画面。
    ///
    /// 调用方必须尽快返回，不要在回调里做重活。
    fn on_frame(&self, data: Vec<u8>, pts_us: u64, capture_ts_us: u64, key: bool);

    /// 收到会话事件。
    fn on_event(&self, event: SessionEvent);
}

/// 发现回调（§3.12 `browse(listener)`）。
#[uniffi::export(callback_interface)]
pub trait DiscoveryListener: Send {
    /// 发现一个主机。
    fn on_host(&self, host: DiscoveredHost);

    /// 主机消失（mDNS 全名）。
    fn on_host_lost(&self, fullname: String);
}

/// Beam 客户端（§3.12 导出面）。
#[derive(uniffi::Object)]
pub struct BeamClient {
    core: CoreClient,
}

#[uniffi::export]
impl BeamClient {
    /// 创建客户端（§3.12 `BeamClient::new(config)`）。
    #[uniffi::constructor]
    pub fn new(config: BeamClientConfig) -> Result<Self, BeamError> {
        let core = CoreClient::new(config.to_core()).map_err(BeamError::from)?;
        Ok(Self { core })
    }

    /// 开始浏览局域网主机（§3.12 `browse(listener)`）。
    ///
    /// 浏览循环在后台线程里跑，事件通过 `listener` 回调给 Kotlin 侧。
    pub fn browse(&self, listener: Box<dyn DiscoveryListener>) -> Result<(), BeamError> {
        let adapter = Box::new(DiscoveryListenerAdapter { inner: listener });
        self.core.browse(adapter).map_err(BeamError::from)
    }

    /// 扫码配对（§3.12 `pair_qr(payload)`）。
    ///
    /// 载荷格式见 §3.3：`beam://pair?h=ip:port&fp=..&t=token`。
    pub fn pair_qr(&self, payload: String) -> Result<Peer, BeamError> {
        let peer = self.core.pair_qr(&payload).map_err(BeamError::from)?;
        Ok(Peer::from_core(&peer))
    }

    /// PIN 配对（§3.12 `pair_pin(peer,pin)`）。
    ///
    /// 只做本地校验（PIN 格式 + 指纹格式）；真正的 SPAKE2 交换由会话层完成。
    pub fn pair_pin(&self, peer: Peer, pin: String) -> Result<(), BeamError> {
        let peer = peer.to_core()?;
        self.core.pair_pin(&peer, &pin).map_err(BeamError::from)
    }

    /// 连接主机（§3.12 `connect(peer,cfg,sink: FrameSink)`）。
    ///
    /// **已知缺口**：内部实现（`crates/beam-ffi/src/client.rs`）目前仍然丢弃
    /// `cfg` 与 `sink`（`let _ = (cfg, sink);`），会话停留在 `Idle`，
    /// [`FrameSink`] 不会收到任何帧或事件。本方法只负责把参数转成内部类型。
    pub fn connect(
        &self,
        peer: Peer,
        cfg: StreamCfg,
        sink: Box<dyn FrameSink>,
    ) -> Result<Arc<Session>, BeamError> {
        let peer = peer.to_core()?;
        let cfg = cfg.to_core()?;
        let adapter = Box::new(FrameSinkAdapter { inner: sink });
        let handle = self
            .core
            .connect(peer, cfg, adapter)
            .map_err(BeamError::from)?;
        Ok(Arc::new(Session { core: handle }))
    }
}

/// 会话句柄（§3.9）。
#[derive(uniffi::Object)]
pub struct Session {
    core: SessionHandle,
}

#[uniffi::export]
impl Session {
    /// 当前会话状态。
    pub fn state(&self) -> SessionState {
        SessionState::from_core(self.core.state())
    }

    /// 主动结束会话（§3.9 `stop()`）。
    pub fn stop(&self) {
        self.core.stop();
    }
}

/// 把导出面的发现回调适配到内部 [`CoreDiscoveryListener`]。
struct DiscoveryListenerAdapter {
    inner: Box<dyn DiscoveryListener>,
}

impl CoreDiscoveryListener for DiscoveryListenerAdapter {
    fn on_host_found(&mut self, host: &CoreDiscoveredHost) {
        self.inner.on_host(DiscoveredHost::from_core(host));
    }

    fn on_host_lost(&mut self, fullname: &str) {
        self.inner.on_host_lost(fullname.to_string());
    }
}

/// 把导出面的帧回调适配到内部 [`CoreFrameSink`]。
///
/// **拷贝路径**：`&[u8]` → `Vec<u8>` 复制一份再交给 Kotlin。
struct FrameSinkAdapter {
    inner: Box<dyn FrameSink>,
}

impl CoreFrameSink for FrameSinkAdapter {
    fn on_frame(&mut self, data: &[u8], pts_us: u64, capture_ts_us: u64, key: bool) {
        self.inner
            .on_frame(data.to_vec(), pts_us, capture_ts_us, key);
    }

    fn on_event(&mut self, event: CoreSessionEvent) {
        self.inner.on_event(SessionEvent::from_core(event));
    }
}

/// 拆分 `host:port`；格式非法返回 `None`。
fn split_address(address: &str) -> Option<(String, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

/// 解析编解码器名（大小写不敏感）。
fn parse_codec(raw: &str) -> Result<CodecKind, BeamError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "h264" | "avc" => Ok(CodecKind::H264),
        "hevc" | "h265" => Ok(CodecKind::Hevc),
        "av1" => Ok(CodecKind::Av1),
        _ => Err(BeamError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_round_trip() {
        let peer = Peer {
            id: String::new(),
            name: String::new(),
            address: "192.168.1.10:4827".to_string(),
            fingerprint: "ab".to_string(),
            platform: String::new(),
            trusted: false,
        };
        let core = peer.to_core().expect("地址应当能解析");
        assert_eq!(core.host, "192.168.1.10");
        assert_eq!(core.port, 4827);
        assert_eq!(core.fingerprint, "ab");
    }

    #[test]
    fn bad_address_is_rejected() {
        let peer = Peer {
            id: String::new(),
            name: String::new(),
            address: "不是地址".to_string(),
            fingerprint: String::new(),
            platform: String::new(),
            trusted: false,
        };
        let err = peer.to_core().expect_err("非法地址必须失败");
        assert!(matches!(err, BeamError::InvalidPayload { .. }));
    }

    #[test]
    fn peer_from_core_keeps_address() {
        let core = CorePeerInfo {
            host: "10.0.0.2".to_string(),
            port: 1234,
            fingerprint: "ff".to_string(),
        };
        let peer = Peer::from_core(&core);
        assert_eq!(peer.address, "10.0.0.2:1234");
        assert_eq!(peer.fingerprint, "ff");
    }

    #[test]
    fn stream_cfg_round_trip() {
        let cfg = StreamCfg {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: "H264".to_string(),
            bitrate_kbps: 20_000,
            fec_ratio: 0.0,
            audio_enabled: true,
        };
        let core = cfg.to_core().expect("应当能转换");
        assert_eq!(core.width, 1920);
        assert_eq!(core.bitrate_bps, 20_000_000);
        assert_eq!(core.codec, CodecKind::H264);
        assert!(core.audio_enabled);
    }

    #[test]
    fn unknown_codec_is_unsupported() {
        let cfg = StreamCfg {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: "vp9".to_string(),
            bitrate_kbps: 20_000,
            fec_ratio: 0.0,
            audio_enabled: false,
        };
        let err = cfg.to_core().expect_err("未知编解码器必须失败");
        assert!(matches!(err, BeamError::Unsupported));
    }

    #[test]
    fn invalid_resolution_is_rejected() {
        let cfg = StreamCfg {
            width: 0,
            height: 1080,
            fps: 60,
            codec: "h264".to_string(),
            bitrate_kbps: 20_000,
            fec_ratio: 0.0,
            audio_enabled: false,
        };
        let err = cfg.to_core().expect_err("非法分辨率必须失败");
        assert!(matches!(err, BeamError::InvalidPayload { .. }));
    }

    #[test]
    fn event_error_carries_code_and_text() {
        let event = SessionEvent::from_core(CoreSessionEvent::Error {
            code: 14001,
            retryable: true,
        });
        match event {
            SessionEvent::Error {
                code,
                message,
                retryable,
            } => {
                assert_eq!(code, "E14001");
                assert!(!message.is_empty());
                assert!(retryable);
            }
            other => panic!("期望 Error 事件，实际 {other:?}"),
        }
    }

    #[test]
    fn state_mapping_is_total() {
        assert_eq!(
            SessionState::from_core(CoreSessionState::Streaming),
            SessionState::Streaming
        );
        assert_eq!(
            SessionState::from_core(CoreSessionState::Error),
            SessionState::Error
        );
    }
}
