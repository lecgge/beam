//! `crates/` 适配器:把 beam-host / beam-client / beam-session / beam-config /
//! beam-log / beam-discovery / beam-pair / beam-net 接到桌面的 [`BeamService`] 接缝上。
//!
//! 接线对应 §4 分层(`apps/desktop → beam-host / beam-client`)与 §3.11 的 command / event 清单。
//! 与 `service.rs` 里的 `LocalService` 的区别是:这里的一切都来自真实的 crates:
//!
//! | 能力 | crates | 说明 |
//! |---|---|---|
//! | 设置持久化 | `beam-config` | 真实读写 `%APPDATA%/Beam/config.toml`;优先级合并与校验走 `merge` / `validate`(§3.14) |
//! | 日志 | `beam-log` | 真实轮转文件日志 `%APPDATA%/Beam/logs`(§3.15) |
//! | 配对 | `beam-pair` | 真实自签证书与指纹;真实二维码载荷 `beam://pair?...`(§3.3) |
//! | 信任库 | `beam-pair` | 真实 `%APPDATA%/Beam/trust.json`;「已配对设备」就是信任库内容 |
//! | 局域网发现 | `beam-discovery` | 真实 mDNS 浏览 `_beam._udp.local` 与广播(§3.4) |
//! | 主机编排 | `beam-host` | 真实 `HostConfig::from_resolved`:设置 → 运行参数(§3.14 → §3.9) |
//! | 客户端编排 | `beam-client` | 真实 `ClientConfig::from_resolved` + `PeerAddr` |
//! | 会话状态机 | `beam-session` | 真实 `StateMachine` 驱动客户端会话状态(§3.9) |
//! | 传输 | `beam-net` | 真实 QUIC 握手(后台进行,结果回报给事件层) |
//!
//! 已知边界(诚实说明):`beam-capture` / `beam-encode` / `beam-decode` / `beam-render` 的
//! 平台后端目前是占位实现(`wgc` / `ffmpeg` / `d3d11` 三个 cargo feature 默认关闭),
//! 因此**画面还没有真正跑起来**;本文件只负责把桌面端接到 crates 的公开 API 上,
//! 缺后端时由 crates 自己返回 E5xxx / E6xxx / E10xxx,原样透传给前端。

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use beam_client::{ClientConfig, ClientRuntime, FrameStatsHandle};
use beam_config::{
    default_config_path, default_trust_path, load_file_layer, merge, save_file_layer, validate,
    AudioMode, ConfigLayers, ResolvedConfig, TomlFileStore,
};
use beam_discovery::{Advertiser, Browser, DiscoveryEvent, HostInfo};
use beam_host::{HostConfig, HostListener, HostRuntime, HostSessionHandle, HostTrustGate};
use beam_pair::{
    default_identity_path, FileKeyStore, Fingerprint, Identity, PairPayload, PairingToken,
    PeerPerms, PeerVerifier, TrustStore, TrustedPeer,
};
use beam_proto::CodecKind;
use beam_session::{PeerAddr, SessionEvent, SessionHandle, SessionState, StateMachine};
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};

/// §3.3：桌面端留了一份「还没配对好」的码（事件层不依赖 `beam-session`）。
///
/// 这里用**编译期断言**保证它与会话层的真值一致 —— 改了一边忘了另一边，
/// 编译就过不去。
const _: () = assert!(beam_session::PAIRING_PENDING_CODE == crate::service::PAIRING_PENDING_CODE);
use crate::dto::*;
use crate::error::*;
use crate::service::{BeamService, SessionEventInfo, TransportOutcome};

/// §3.3 `pin_ttl=120s`。
const PIN_TTL: Duration = Duration::from_secs(120);

/// §3.4 的 mDNS TXT 不带平台字段,发现到的主机平台无从得知。
///
/// 诚实标注为「未知平台」,而不是编一个。
const PLATFORM_UNKNOWN: &str = "未知平台";

/// 桌面端自有、beam-config 未建模的设置项。
///
/// 持久化在 `%APPDATA%/Beam/desktop.json`;
/// beam-config 负责 §3.14 的通用配置面(画质 / 网络 / 输入 / 音频)。
///
/// **只保留桌面端真正的专属项**(窗口行为 / 主题语言 / 端口等)。
/// 抓屏后端(`capture_backend`)、切片(`slices`)、SFE(`sfe`)已经是
/// `beam-config` 的规范配置项(§3.5 / §3.6),桌面端不再自持一份,
/// 一律读写 `ResolvedConfig`,避免两处真值。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSettings {
    pub match_client_resolution: bool,
    pub preset: String,
    pub port: u16,
    pub allow_tailscale: bool,
    pub language: String,
    pub theme: String,
    pub show_stats_overlay: bool,
    pub minimize_to_tray: bool,
    pub launch_on_startup: bool,
    /// 客户端是否把本机键鼠回传给主机(§3.8)。默认关(敏感操作,用户主动开启)。
    pub share_input: bool,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            match_client_resolution: true,
            preset: "low_latency_p1".to_string(),
            port: 47990,
            allow_tailscale: false,
            language: "zh-CN".to_string(),
            theme: "dark".to_string(),
            show_stats_overlay: true,
            minimize_to_tray: true,
            launch_on_startup: false,
            share_input: false,
        }
    }
}

/// 客户端会话的接线选项（§3.3 PIN / §3.8 键鼠回传）。
#[derive(Debug, Clone, Default)]
struct ClientSessionOptions {
    /// 客户端输入的 PIN（§3.3）：有值才发起 SPAKE2 配对。
    pin: Option<String>,
    /// 是否把本机键鼠回传给主机（§3.8）。
    share_input: bool,
}

/// mDNS 浏览结果(由独立线程维护)。
#[derive(Debug, Default)]
struct DiscoveryState {
    hosts: Mutex<Vec<beam_discovery::DiscoveredHost>>,
}

impl DiscoveryState {
    fn upsert(&self, host: beam_discovery::DiscoveredHost) {
        let mut guard = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        match guard.iter_mut().find(|h| h.id == host.id) {
            Some(existing) => *existing = host,
            None => guard.push(host),
        }
    }

    fn remove(&self, fullname: &str) {
        let mut guard = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|h| h.fullname != fullname);
    }

    fn snapshot(&self) -> Vec<beam_discovery::DiscoveredHost> {
        self.hosts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// 桌面端状态。
struct CrateInner {
    host: HostStatus,
    layers: ConfigLayers,
    resolved: ResolvedConfig,
    ui: DesktopSettings,
    identity: Identity,
    /// §3.3 信任库。
    ///
    /// 用 `Arc<Mutex<>>` 是因为它要与**主机侧信任闸门**
    /// ([`HostTrustGate`])共享同一份:用户在弹窗里点了「同意」写库之后,
    /// 闸门**立刻**就能看到 —— 不需要重建监听,也不需要重新握手。
    trust: Arc<Mutex<TrustStore>>,
    state: StateMachine,
    advertiser: Option<Advertiser>,
    session: Option<SessionInfo>,
    /// 真实客户端会话句柄(§3.9):`disconnect` 用它结束会话。
    client_session: Option<SessionHandle>,
    /// 帧通道统计句柄(§3.2):E2004(帧过期丢弃)的计数从这里读。
    frame_stats: Option<FrameStatsHandle>,
    /// 待确认的配对请求(§3.3)。
    ///
    /// 客户端侧与**主机侧**的请求共用这一个列表:两者都是
    /// 「用户确认后才写信任库」,前端用同一张卡片呈现(§3.11 `pair://request`)。
    pending_pairs: Vec<PairRequest>,
    /// 自上次上报以来**新出现**的待确认请求(供 `pair://request` 事件)。
    new_pairs: Vec<PairRequest>,
    /// 已被用户拒绝的主机指纹:发现列表不再把它们当成待确认请求。
    rejected_pairs: Vec<String>,
    /// 正在采集完整指纹的主机 id(避免重复发起探测)。
    probing: Vec<String>,
    /// 当前配对码(含真实二维码载荷与轮换时间,§3.3)。
    pairing: PairingCode,
    /// 当前二维码载荷里的**一次性 token**（§3.3 `pin_ttl=120s`）。
    ///
    /// 主机把它放进会话层的 `PairingConfig.token` ——
    /// 扫码那条路（Android 扫二维码）走 `PairRequest{token}`（§5），
    /// 不需要用户手输 PIN。二维码载荷里已经带着它，这里单独存一份
    /// 免得每次都去解析 URI。
    host_pairing_token: Option<String>,
    config_path: Option<PathBuf>,
    trust_path: Option<PathBuf>,
    desktop_path: Option<PathBuf>,
    /// 主机侧 QUIC 监听(§3.2 / §3.9)。
    ///
    /// §3.11 `start_hosting` 之后存在,`stop_hosting` 时关闭。
    host_listener: Option<Arc<HostListener>>,
    /// 主机侧信任闸门(产品方案 §8)。
    ///
    /// 未写入信任库的对端**一律拒绝握手**,但指纹被记住;
    /// 用户人工确认写库后,下一次握手才放行。
    host_gate: Option<Arc<HostTrustGate>>,
    /// 正在服务中的主机会话(客户端接入之后)。
    ///
    /// 用 `Arc` 包一层:[`HostSessionHandle`] 不可克隆,但需要同时被
    /// 「状态槽」和「推进循环」持有。
    host_session: Option<Arc<HostSessionHandle>>,
    /// §3.11 `session://reconnecting` 是否已经报过（进入重连态时报一次）。
    ///
    /// 「取走即清空」的语义靠它实现：重连期间不重复报，离开重连态后重新武装。
    reconnect_notice_sent: bool,
    /// §3.11 `session://waiting-approval` 是否已经报过（只报一次）。
    ///
    /// 客户端在等主机点「同意」时会**每秒重试一次**（§4.1），
    /// 不去重的话提示会刷屏。
    waiting_approval_notice: bool,
    /// PIN 尝试预算(§3.3 `max_attempts=3`):按主机 id 记账。
    pin_budget: crate::pin::AttemptBudget,
}

/// crates/ 适配器。
pub struct CrateService {
    inner: Arc<Mutex<CrateInner>>,
    discovery: Arc<DiscoveryState>,
    /// 传输层握手结果槽:后台任务写入,事件层取走。
    transport_outcome: Arc<Mutex<Option<TransportOutcome>>>,
    /// 真实客户端会话的事件槽:后台会话任务写入,事件层取走。
    session_events: Arc<Mutex<Vec<SessionEventInfo>>>,
}

impl CrateService {
    pub fn new() -> Self {
        let config_path = default_config_path();
        let trust_path = default_trust_path();
        let desktop_path = config_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|dir| dir.join("desktop.json"));

        // §3.14:读取用户配置文件层。
        let layers = match &config_path {
            Some(path) => {
                let store = TomlFileStore::new(path);
                match load_file_layer(&store) {
                    Ok(layers) => layers,
                    Err(err) => {
                        tracing::warn!(code = %err.code_str(), "读取配置文件失败,使用默认配置");
                        ConfigLayers::default()
                    }
                }
            }
            None => ConfigLayers::default(),
        };
        let resolved = merge(&layers);

        // 桌面端自有设置。
        let ui = desktop_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<DesktopSettings>(&text).ok())
            .unwrap_or_default();

        // §3.3:本机身份(自签证书 + 指纹),落在 %APPDATA%/Beam/identity.key。
        let identity = match default_identity_path() {
            Some(path) => {
                let store = FileKeyStore::new(path);
                match Identity::load_or_create(&store) {
                    Ok(id) => id,
                    Err(err) => {
                        tracing::error!(code = %err.code_str(), "加载本机身份失败");
                        Identity::generate().expect("生成自签身份不应失败")
                    }
                }
            }
            None => Identity::generate().expect("生成自签身份不应失败"),
        };

        // §3.3:信任库。`Arc<Mutex<>>` 便于与主机侧信任闸门共享同一份。
        let trust = match &trust_path {
            Some(path) => TrustStore::load(path).unwrap_or_else(|err| {
                tracing::warn!(code = %err.code_str(), "读取信任库失败,按空库处理");
                TrustStore::new()
            }),
            None => TrustStore::new(),
        };
        let trust = Arc::new(Mutex::new(trust));

        let device_name = device_name();
        let mut host = HostStatus {
            state: HostState::Idle,
            device_name: device_name.clone(),
            local_ip: detect_local_ip(),
            port: ui.port,
            gpu: None,
            encoder: Some(resolved.video.encoder.to_uppercase()),
            codec: Some(resolved.video.codec.to_uppercase()),
            resolution: resolution_label(resolved.video.width, resolved.video.height),
            fps: resolved.video.fps,
            bitrate_mbps: resolved.video.bitrate_bps / 1_000_000,
            discovery_enabled: resolved.network.discovery_enabled,
            connected_device: None,
            message: None,
            backend: BackendKind::BeamCrates,
            revision: 0,
            updated_at_ms: now_ms(),
        };
        host.revision = 1;

        let discovery = Arc::new(DiscoveryState::default());
        spawn_discovery_browser(discovery.clone());

        // §3.3:初始配对码。
        let (pairing, pairing_token) = build_pairing(&identity, host.local_ip.as_deref(), ui.port);

        Self {
            inner: Arc::new(Mutex::new(CrateInner {
                host,
                layers,
                resolved,
                ui,
                identity,
                trust,
                state: StateMachine::new(),
                advertiser: None,
                session: None,
                client_session: None,
                frame_stats: None,
                pending_pairs: Vec::new(),
                new_pairs: Vec::new(),
                rejected_pairs: Vec::new(),
                probing: Vec::new(),
                pairing,
                host_pairing_token: Some(pairing_token),
                config_path,
                trust_path,
                desktop_path,
                host_listener: None,
                host_gate: None,
                host_session: None,
                reconnect_notice_sent: false,
                waiting_approval_notice: false,
                pin_budget: crate::pin::AttemptBudget::new(),
            })),
            discovery,
            transport_outcome: Arc::new(Mutex::new(None)),
            session_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CrateInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 把「设置 → 运行参数」交给 beam-host 走一遍(§3.14 → §3.9)。
    fn host_stream_config(&self, inner: &CrateInner) -> beam_host::HostConfig {
        HostConfig::from_resolved(&inner.resolved)
    }

    /// 生成配对码与真实二维码载荷(§3.3)。
    ///
    /// 返回 `(配对码, 一次性 token)` —— token 单独给主机侧放进
    /// 会话层的 `PairingConfig.token`(§5 `PairRequest{token}`)。
    fn build_pairing(&self, inner: &CrateInner) -> (PairingCode, String) {
        build_pairing(
            &inner.identity,
            inner.host.local_ip.as_deref(),
            inner.ui.port,
        )
    }
}

impl BeamService for CrateService {
    fn backend(&self) -> BackendKind {
        BackendKind::BeamCrates
    }

    fn host_status(&self) -> HostStatus {
        self.lock().host.clone()
    }

    fn start_hosting(&self, opts: StartHostingOptions) -> Result<HostStatus, BeamError> {
        let mut inner = self.lock();
        if inner.host.state == HostState::Hosting || inner.host.state == HostState::Streaming {
            return Err(BeamError::invalid(
                E_SESSION_ALREADY_ACTIVE,
                "已经在串流中,先停止再重新开播",
            ));
        }

        // §3.14:会话内临时覆盖优先于用户配置文件。
        let mut layers = inner.layers.clone();
        if let Some(res) = &opts.resolution {
            if let Some((w, h)) = resolution_dims(res) {
                layers.session.video.width = Some(w);
                layers.session.video.height = Some(h);
            }
        }
        if let Some(fps) = opts.fps {
            layers.session.video.fps = Some(fps);
        }
        if let Some(codec) = &opts.codec {
            layers.session.video.codec = Some(codec.clone());
        }
        if let Some(bitrate) = opts.bitrate_mbps {
            layers.session.video.bitrate_bps = Some(bitrate * 1_000_000);
        }
        if let Some(audio) = &opts.audio_mode {
            layers.session.audio.mode = Some(match audio.as_str() {
                "host+stream" => AudioMode::HostAndStream,
                "stream_only" => AudioMode::StreamOnly,
                _ => AudioMode::Off,
            });
        }
        let resolved = merge(&layers);
        validate(&resolved).map_err(|err| {
            // 校验失败来自 `beam-config`(§3.14),**透传它自己的码**(E11002),
            // 而不是外壳的码 —— `E11xxx` 归配置层独占(S5 裁决)。
            BeamError::invalid(
                &err.code_str(),
                format!("配置校验失败 {}: {err}", err.code_str()),
            )
        })?;

        // §3.14 / §3.9:设置 → 主机运行参数。
        let host_cfg = HostConfig::from_resolved(&resolved);
        inner.host.resolution = resolution_label(host_cfg.stream.width, host_cfg.stream.height);
        inner.host.fps = host_cfg.stream.fps;
        inner.host.bitrate_mbps = host_cfg.stream.bitrate_bps / 1_000_000;
        inner.host.codec = Some(resolved.video.codec.to_uppercase());
        inner.host.encoder = Some(resolved.video.encoder.to_uppercase());

        // §3.4:真实 mDNS 广播。组播被环境阻断时只告警,不影响开播本身。
        let ip: Ipv4Addr = inner
            .host
            .local_ip
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(Ipv4Addr::UNSPECIFIED);
        let info = HostInfo {
            // 桌面端是 Windows 主机/客户端，广播自己的平台（S1 裁决）。
            platform: "windows".to_string(),
            id: inner.identity.fingerprint().short_hex(),
            name: inner.host.device_name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            fingerprint: inner.identity.fingerprint().to_hex(),
            port: inner.ui.port,
            address: ip,
        };
        match Advertiser::start(info) {
            Ok(advertiser) => {
                inner.advertiser = Some(advertiser);
                inner.host.message = None;
            }
            Err(err) => {
                tracing::warn!(code = %err.code_str(), "mDNS 广播失败,主机不会出现在局域网列表里");
                inner.host.message = Some(format!(
                    "局域网广播失败({}),主机只能手动输入 IP 连接",
                    err.code_str()
                ));
            }
        }

        inner.host.state = HostState::Hosting;
        inner.host.revision += 1;
        inner.host.updated_at_ms = now_ms();

        // §3.9 / §3.2:真起一个 QUIC 监听。
        //
        // 之前这里只广播 mDNS —— 「已开播的主机」并不会真的接受客户端接入。
        // 现在起监听,并在后台跑接受循环(见 `spawn_host_listener`)。
        //
        // 监听是**异步**的(要建 QUIC 端点),而 `start_hosting` 是同步 command
        // (跑在 UI 线程上),所以这里只做「安排」,监听结果由后台任务写回状态。
        let resolved = inner.resolved.clone();
        let identity = inner.identity.clone();
        let trust = Arc::clone(&inner.trust);
        let port = inner.ui.port;
        let bind_ip = inner
            .host
            .local_ip
            .as_deref()
            .and_then(|s| s.parse::<Ipv4Addr>().ok())
            .unwrap_or(Ipv4Addr::UNSPECIFIED);
        drop(inner);
        self.spawn_host_listener(resolved, identity, trust, bind_ip, port);

        let mut inner = self.lock();
        inner.host.state = HostState::Hosting;
        inner.host.revision += 1;
        inner.host.updated_at_ms = now_ms();
        Ok(inner.host.clone())
    }

    fn stop_hosting(&self) -> Result<HostStatus, BeamError> {
        let mut inner = self.lock();
        if inner.host.state == HostState::Idle {
            return Err(BeamError::invalid(
                E_SESSION_INVALID_STATE,
                "当前没有在串流",
            ));
        }
        inner.advertiser = None;
        // §3.11 `stop_hosting`:关掉 QUIC 监听与正在服务的主机会话。
        if let Some(listener) = inner.host_listener.take() {
            listener.close();
        }
        inner.host_gate = None;
        if let Some(session) = inner.host_session.take() {
            // `HostSessionHandle` 的 `Drop` 会取消输入泵;这里再显式停一次会话,
            // 让对端收到 `Bye`(§5)。
            session.session.stop();
        }
        inner.host.state = HostState::Idle;
        inner.host.connected_device = None;
        inner.host.message = None;
        inner.host.revision += 1;
        inner.host.updated_at_ms = now_ms();
        Ok(inner.host.clone())
    }

    fn pairing_code(&self) -> PairingCode {
        let inner = self.lock();
        inner.pairing.clone()
    }

    fn approve_pair(&self, id: &str) -> Result<Device, BeamError> {
        let mut inner = self.lock();
        let fp = Fingerprint::from_hex(id).map_err(|err| {
            BeamError::invalid(
                E_FINGERPRINT_MISMATCH,
                format!("指纹非法 {}: {err}", err.code_str()),
            )
        })?;
        let pending = inner.pending_pairs.iter().find(|r| r.id == id).cloned();
        let peer = TrustedPeer {
            fp,
            name: pending
                .as_ref()
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "未知设备".to_string()),
            platform: pending
                .as_ref()
                .map(|r| r.platform.clone())
                .unwrap_or_else(|| PLATFORM_UNKNOWN.to_string()),
            paired_at: now_ms() / 1000,
            perms: PeerPerms {
                view: true,
                input: true,
                audio: true,
                gamepad: true,
            },
        };
        // 真实写入信任库(§3.3):人工确认之后才写(产品方案 §8)。
        //
        // 写的是**共享**信任库:主机侧信任闸门(§3.3)拿到的是同一份,
        // 因此用户点了「同意」之后,下一次握手立刻就能通过 —— 不必重启监听。
        {
            let mut trust = inner.trust.lock().unwrap_or_else(|e| e.into_inner());
            trust.upsert(peer);
        }
        inner.pending_pairs.retain(|r| r.id != id);
        inner.new_pairs.retain(|r| r.id != id);
        self.save_trust(&inner)?;
        let peer = inner
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .last()
            .cloned();
        Ok(device_from_peer(&peer.expect("刚刚 upsert 过")))
    }

    fn reject_pair(&self, id: &str) -> Result<(), BeamError> {
        let mut inner = self.lock();
        let before = inner.pending_pairs.len();
        inner.pending_pairs.retain(|r| r.id != id);
        inner.new_pairs.retain(|r| r.id != id);
        if inner.pending_pairs.len() == before {
            return Err(BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("配对请求 {id} 不存在"),
            ));
        }
        // 记住被拒绝的主机,发现列表不再把它当成待确认请求。
        let prefix: String = id.chars().take(8).collect();
        if !prefix.is_empty() && !inner.rejected_pairs.contains(&prefix) {
            inner.rejected_pairs.push(prefix);
        }
        Ok(())
    }

    fn remove_device(&self, id: &str) -> Result<(), BeamError> {
        let inner = self.lock();
        let fp = Fingerprint::from_hex(id).map_err(|err| {
            BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("指纹非法 {}: {err}", err.code_str()),
            )
        })?;
        let removed = inner
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&fp);
        if !removed {
            return Err(BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("设备 {id} 不在信任库里"),
            ));
        }
        // 真实写回信任库文件。
        self.save_trust(&inner)?;
        Ok(())
    }

    fn devices(&self) -> Vec<Device> {
        let inner = self.lock();
        let trust = inner.trust.lock().unwrap_or_else(|e| e.into_inner());
        trust.peers().iter().map(device_from_peer).collect()
    }

    fn pair_requests(&self) -> Vec<PairRequest> {
        self.lock().pending_pairs.clone()
    }

    fn list_hosts(&self) -> Vec<DiscoveredHost> {
        let inner = self.lock();
        let trusted: Vec<String> = inner
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .iter()
            .map(|p| p.fp.short_hex().to_lowercase())
            .collect();
        self.discovery
            .snapshot()
            .iter()
            .map(|h| DiscoveredHost {
                id: h.id.clone(),
                name: h.name.clone(),
                address: h.address.to_string(),
                port: h.port,
                trusted: trusted.contains(&h.fingerprint.to_lowercase()),
                version: h.version.clone(),
                gpu: None,
                online: true,
                platform: h.platform.clone(),
            })
            .collect()
    }

    fn connect(&self, host_id: &str, pin: &str) -> Result<SessionInfo, BeamError> {
        let mut inner = self.lock();
        if inner.session.is_some() {
            return Err(BeamError::invalid(
                E_SESSION_ALREADY_ACTIVE,
                "已经有一条串流会话,请先断开",
            ));
        }
        let host = self
            .discovery
            .snapshot()
            .into_iter()
            .find(|h| h.id == host_id)
            .ok_or_else(|| {
                BeamError::retryable(
                    E_HOST_NOT_FOUND,
                    format!("未发现主机 {host_id},请刷新后重试"),
                )
            })?;

        // §3.9 / §3.14:客户端运行参数来自解析后的配置。
        let client_cfg = ClientConfig::from_resolved(&inner.resolved);
        // §3.4 的 mDNS 只广播指纹前 8 位;信任库里存的是完整指纹,
        // 这里用前缀匹配出完整指纹交给会话层(真正的校验由 TLS 信任库完成)。
        let discovered_prefix = host.fingerprint.to_lowercase();
        let trusted_fingerprint = inner
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .iter()
            .find(|p| p.fp.short_hex().to_lowercase() == discovered_prefix)
            .map(|p| p.fp.to_hex());

        // §3.3:SPAKE2 的密码学校验在主机侧(§3.3),尚未接线;
        // 这一层先保证 PIN **参与**连接流程:格式合法 + 受尝试预算约束。
        //
        // 已在信任库里的主机(有完整指纹)不需要再输 PIN。
        let trusted = trusted_fingerprint.is_some();
        let now = Instant::now();
        if let Err(err) =
            crate::pin::check_connect_pin(&inner.pin_budget, host_id, trusted, pin, now)
        {
            // 格式错误算一次尝试(§3.3 `max_attempts=3`),锁定则如实报 E3002。
            if err.code == E_PIN_INVALID {
                if let Err(locked) = inner.pin_budget.record_failure(host_id, now) {
                    return Err(BeamError::invalid(E_PIN_LOCKED, locked));
                }
            }
            return Err(err);
        }
        let peer = PeerAddr {
            host: host.address.to_string(),
            port: host.port,
            fingerprint: Some(trusted_fingerprint.unwrap_or(host.fingerprint.clone())),
        };
        let _ = client_cfg.peer.clone();

        // §3.9:真实状态机推进。
        for next in [
            SessionState::Discovering,
            SessionState::Pairing,
            SessionState::Connecting,
            SessionState::Negotiating,
        ] {
            inner
                .state
                .transition(next)
                .map_err(|err| BeamError::invalid(E_SESSION_INVALID_STATE, err.text()))?;
        }

        let now = now_ms();
        let session = SessionInfo {
            session_id: format!("s-{:x}", now),
            host_id: host.id.clone(),
            host_name: host.name.clone(),
            state: "negotiating".to_string(),
            resolution: resolution_label(client_cfg.stream.width, client_cfg.stream.height),
            fps: client_cfg.stream.fps,
            codec: codec_name(client_cfg.stream.codec),
            started_at_ms: now,
        };
        // PIN 准入通过 ⇒ 该主机的尝试预算复位(§3.3)。
        inner.pin_budget.reset(host_id);
        inner.session = Some(session.clone());

        // §3.9:真实客户端会话(控制面 + 帧面)。建链在后台进行,
        // 结果通过会话事件与传输结果回报给事件层。
        let identity = inner.identity.clone();
        // 客户端握手用的是信任库的一份**快照**:握手期间信任库不会变,
        // 真正的写库发生在 `approve_pair`(§3.3)。
        let trust = inner
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let resolved = inner.resolved.clone();
        // §3.3：客户端输入的 PIN **真的进到会话层** —— 会话层拿它发起
        // SPAKE2 配对（§4.1「PairRequest(token 或 SPAKE2 msg)」），
        // 主机侧用同一个 PIN 校验，通过之后才进串流。
        // 已配对的主机传空串 ⇒ 不配对（对端已在信任库里）。
        let pin_for_pairing = if pin.is_empty() {
            None
        } else {
            Some(pin.to_string())
        };
        // §3.8：客户端是否把本机键鼠回传给主机。
        let share_input = inner.ui.share_input;
        self.spawn_client_session(
            resolved,
            peer,
            host.name.clone(),
            identity,
            trust,
            ClientSessionOptions {
                pin: pin_for_pairing,
                share_input,
            },
        );

        Ok(session)
    }

    fn disconnect(&self) -> Result<SessionInfo, BeamError> {
        let mut inner = self.lock();
        let session = inner
            .session
            .take()
            .ok_or_else(|| BeamError::invalid(E_SESSION_INVALID_STATE, "当前没有串流会话"))?;
        // 结束真实会话(§3.9):会话层会发 `Bye` 再释放传输。
        if let Some(handle) = inner.client_session.take() {
            handle.stop();
        }
        inner.frame_stats = None;
        let _ = inner.state.transition(SessionState::Closed);
        Ok(session)
    }

    fn session(&self) -> Option<SessionInfo> {
        self.lock().session.clone()
    }

    fn session_stats(&self) -> Option<SessionStats> {
        let inner = self.lock();
        let session = inner.session.clone()?;
        // **真实**的会话统计（§3.9）：客户端会话周期上报 `StatsReport`
        // （fps / 码率 / RTT / 丢包率），`beam-session` 把它汇总成
        // `SessionSummary`。此前这里用的是「协商后的配置」——那是**请求值**
        // 不是**实测值**（帧率写死成目标帧率、码率写死成配置码率）。
        let summary = inner.client_session.as_ref().map(|handle| handle.summary());
        let fps = summary.as_ref().map(|s| s.fps).unwrap_or(0);
        let bitrate_bps = summary.as_ref().map(|s| s.bitrate_bps).unwrap_or(0);
        let rtt_us = summary.as_ref().map(|s| s.rtt_us).unwrap_or(0);
        let loss_ppm = summary.as_ref().map(|s| s.loss_ppm).unwrap_or(0);
        let dropped_frames = match inner.frame_stats.as_ref() {
            Some(handle) => handle.snapshot().frames_expired as u32,
            // 没有帧通道统计时退回会话层的丢帧计数（也是真实的）。
            None => summary
                .as_ref()
                .map(|s| s.dropped_frames as u32)
                .unwrap_or(0),
        };
        Some(SessionStats {
            session_id: session.session_id.clone(),
            // 各**阶段**耗时（抓屏 / 颜色转换 / 编码 / 解码 / 渲染 / E2E）
            // 只有 `beam-perf` 的采集器知道，而桌面端默认不开 `perf-stats`
            // （§7）⇒ 这里如实留 0，前端按「未采集」显示。
            e2e_ms: 0.0,
            capture_ms: 0.0,
            color_convert_ms: 0.0,
            encode_ms: 0.0,
            // 网络耗时用**真实**的往返时延（RTT）。
            network_ms: rtt_us as f32 / 1000.0,
            decode_ms: 0.0,
            render_ms: 0.0,
            fps,
            bitrate_mbps: bitrate_bps as f32 / 1_000_000.0,
            // 百万分比 → 百分比。
            loss_pct: loss_ppm as f32 / 10_000.0,
            fec_recovered_pct: 0.0,
            jitter_ms: 0.0,
            dropped_frames,
            resolution: session.resolution.clone(),
            codec: session.codec.clone(),
            timestamp_ms: now_ms(),
        })
    }

    fn reconnect_notice(&self) -> Option<ReconnectingInfo> {
        // §3.11 `session://reconnecting`：由会话层的**真实**重连状态驱动，不合成。
        //
        // `beam_session::SessionHandle::reconnecting()` 在会话进入
        // `Reconnecting` 时返回 `Some(ReconnectingInfo{elapsed_ms, keep_last_frame})`，
        // 离开重连态后返回 `None`。此前这里恒为 `None`（那是因为会话层还没有
        // 重连状态机），于是产品方案 §3 第 5 步的「3 秒无帧显示重连中」
        // 在桌面上永远看不到。
        //
        // 「取走即清空」的语义：**只在进入重连态的那一次**报；重连期间不重复刷屏；
        // 离开重连态后重新武装，下一次重连还能再报。
        let mut inner = self.lock();
        let info = inner.client_session.as_ref().and_then(|h| h.reconnecting());
        match info {
            Some(info) => {
                if inner.reconnect_notice_sent {
                    return None;
                }
                inner.reconnect_notice_sent = true;
                Some(ReconnectingInfo {
                    session_id: inner
                        .session
                        .as_ref()
                        .map(|s| s.session_id.clone())
                        .unwrap_or_default(),
                    elapsed_ms: info.elapsed_ms,
                    keep_last_frame: info.keep_last_frame,
                })
            }
            None => {
                inner.reconnect_notice_sent = false;
                None
            }
        }
    }

    fn settings(&self) -> Settings {
        let inner = self.lock();
        settings_from(&inner.resolved, &inner.ui)
    }

    fn set_settings(&self, patch: SettingsPatch) -> Result<Settings, BeamError> {
        let mut inner = self.lock();

        // ---- beam-config 负责的字段(§3.14 的 file 层)----
        let mut layers = inner.layers.clone();
        if let Some(res) = &patch.resolution {
            let (w, h) = resolution_dims(res).ok_or_else(|| {
                BeamError::invalid(E_SHELL_CONFIG_INVALID, format!("不支持的分辨率 {res}"))
            })?;
            layers.file.video.width = Some(w);
            layers.file.video.height = Some(h);
        }
        if let Some(fps) = patch.fps {
            layers.file.video.fps = Some(fps);
        }
        if let Some(bitrate) = patch.bitrate_mbps {
            layers.file.video.bitrate_bps = Some(bitrate * 1_000_000);
        }
        if let Some(hdr) = patch.hdr {
            layers.file.video.hdr = Some(hdr);
        }
        if let Some(codec) = &patch.codec {
            layers.file.video.codec = Some(codec.clone());
        }
        if let Some(encoder) = &patch.encoder {
            layers.file.video.encoder = Some(encoder.clone());
        }
        // 规范配置项(§3.5 / §3.6):抓屏后端 / 切片 / SFE 一律写进 beam-config,
        // 桌面端不再维护平行的一份。
        if let Some(backend) = &patch.capture_backend {
            layers.file.capture.backend = Some(backend.clone());
        }
        if let Some(slices) = patch.slices {
            layers.file.video.slices = Some(slices);
        }
        if let Some(sfe) = patch.sfe {
            layers.file.video.sfe = Some(sfe);
        }
        if let Some(ratio) = patch.fec_ratio {
            layers.file.network.fec_ratio = Some(ratio);
        }
        if let Some(min) = patch.bitrate_min_mbps {
            layers.file.network.bitrate_min = Some(min * 1_000_000);
        }
        if let Some(max) = patch.bitrate_max_mbps {
            layers.file.network.bitrate_max = Some(max * 1_000_000);
        }
        if let Some(discovery) = patch.discovery_enabled {
            layers.file.network.discovery_enabled = Some(discovery);
        }
        if let Some(gamepad) = patch.audio_mode.as_ref().map(|_| false).or(None) {
            layers.file.input.gamepad = Some(gamepad);
        }
        if let Some(mode) = &patch.audio_mode {
            layers.file.audio.mode = Some(match mode.as_str() {
                "host+stream" => AudioMode::HostAndStream,
                "stream_only" => AudioMode::StreamOnly,
                _ => AudioMode::Off,
            });
        }
        if let Some(bitrate) = patch.audio_bitrate_kbps {
            layers.file.audio.bitrate_bps = Some(bitrate * 1000);
        }

        let resolved = merge(&layers);
        validate(&resolved).map_err(|err| {
            // 校验失败来自 `beam-config`(§3.14),**透传它自己的码**(E11002),
            // 而不是外壳的码 —— `E11xxx` 归配置层独占(S5 裁决)。
            BeamError::invalid(
                &err.code_str(),
                format!("配置校验失败 {}: {err}", err.code_str()),
            )
        })?;

        // ---- 桌面端自有字段 ----
        let mut ui = inner.ui.clone();
        if let Some(v) = patch.match_client_resolution {
            ui.match_client_resolution = v;
        }
        if let Some(v) = patch.preset {
            ui.preset = v;
        }
        if let Some(v) = patch.port {
            ui.port = v;
        }
        if let Some(v) = patch.allow_tailscale {
            ui.allow_tailscale = v;
        }
        if let Some(v) = patch.language {
            ui.language = v;
        }
        if let Some(v) = patch.theme {
            ui.theme = v;
        }
        if let Some(v) = patch.show_stats_overlay {
            ui.show_stats_overlay = v;
        }
        if let Some(v) = patch.minimize_to_tray {
            ui.minimize_to_tray = v;
        }
        if let Some(v) = patch.launch_on_startup {
            ui.launch_on_startup = v;
        }
        if let Some(v) = patch.share_input {
            ui.share_input = v;
        }

        // 真实落盘:%APPDATA%/Beam/config.toml + desktop.json。
        if let Some(path) = inner.config_path.clone() {
            let store = TomlFileStore::new(path);
            save_file_layer(&store, &layers).map_err(|err| {
                // 写配置失败来自 `beam-config` 的存储层 ⇒ 透传它的码(E11003)。
                BeamError::new(
                    &err.code_str(),
                    format!("写入配置失败 {}: {err}", err.code_str()),
                    false,
                )
            })?;
        }
        if let Some(path) = inner.desktop_path.clone() {
            let text = serde_json::to_string_pretty(&ui).map_err(|err| {
                BeamError::new(E_SHELL_IO, format!("序列化桌面设置失败: {err}"), false)
            })?;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, text).map_err(|err| {
                BeamError::new(E_SHELL_IO, format!("写入桌面设置失败: {err}"), false)
            })?;
        }

        inner.layers = layers;
        inner.resolved = resolved;
        inner.ui = ui;
        inner.host.resolution =
            resolution_label(inner.resolved.video.width, inner.resolved.video.height);
        inner.host.fps = inner.resolved.video.fps;
        inner.host.bitrate_mbps = inner.resolved.video.bitrate_bps / 1_000_000;
        inner.host.codec = Some(inner.resolved.video.codec.to_uppercase());
        inner.host.encoder = Some(inner.resolved.video.encoder.to_uppercase());
        inner.host.discovery_enabled = inner.resolved.network.discovery_enabled;
        inner.host.port = inner.ui.port;
        inner.host.revision += 1;
        inner.host.updated_at_ms = now_ms();

        Ok(settings_from(&inner.resolved, &inner.ui))
    }

    fn perf_snapshot(&self) -> PerfSnapshot {
        #[cfg(feature = "perf-stats")]
        {
            // §3.13：真实数据来自 `beam-perf` 的全局采集器
            // （平台层的抓屏 / 颜色转换 / 编码 / 解码 / 渲染埋点）。
            let snapshot = beam_perf::Perf::snapshot();
            let stages = snapshot
                .stages
                .iter()
                .map(|(stage, s)| PerfStageStat {
                    stage: stage.as_str().to_string(),
                    mean_ms: s.mean_ns as f32 / 1_000_000.0,
                    p50_ms: s.p50_ns as f32 / 1_000_000.0,
                    p99_ms: s.p99_ns as f32 / 1_000_000.0,
                })
                .collect();
            let mut metrics = vec![
                PerfMetric {
                    name: "fps".to_string(),
                    value: snapshot.fps.to_string(),
                    unit: "fps".to_string(),
                },
                PerfMetric {
                    name: "bitrate".to_string(),
                    value: format!("{:.2}", snapshot.bitrate_bps as f64 / 1_000_000.0),
                    unit: "Mbps".to_string(),
                },
                PerfMetric {
                    name: "dropped_frames".to_string(),
                    value: snapshot.dropped_frames.to_string(),
                    unit: "frames".to_string(),
                },
            ];
            // CPU / GPU / 内存：**没有真实采样源时整项不写** ——
            // 写成 0 会让「未采集」冒充「实测为 0」。
            if let Some(cpu) = snapshot.cpu_percent() {
                metrics.push(PerfMetric {
                    name: "cpu".to_string(),
                    value: format!("{cpu:.1}"),
                    unit: "%".to_string(),
                });
            }
            if let Some(gpu) = snapshot.gpu_percent() {
                metrics.push(PerfMetric {
                    name: "gpu".to_string(),
                    value: format!("{gpu:.1}"),
                    unit: "%".to_string(),
                });
            }
            if let Some(mem) = snapshot.memory_used_bytes() {
                metrics.push(PerfMetric {
                    name: "memory".to_string(),
                    value: format!("{:.1}", mem as f64 / (1024.0 * 1024.0)),
                    unit: "MB".to_string(),
                });
            }
            PerfSnapshot {
                available: true,
                stages,
                metrics,
                // 端到端**序列**：beam-perf 只保留各阶段的统计量
                // （count/mean/p50/p95/p99/max），不保留逐点序列 ⇒ 这里如实为空，
                // 不用「只有最后一个点」之类的假序列去糊。
                e2e_series: Vec::new(),
                samples: snapshot.samples as u32,
                captured_at_ms: now_ms(),
            }
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            // §3.13 门控：`perf-stats` 没开 ⇒ 没有采集器 ⇒ 如实报「不可用」，
            // 而不是返回一组 0（那会被前端当成「实测为 0」）。
            PerfSnapshot {
                available: false,
                stages: Vec::new(),
                metrics: Vec::new(),
                e2e_series: Vec::new(),
                samples: 0,
                captured_at_ms: now_ms(),
            }
        }
    }

    fn perf_tick(&self) -> Option<PerfTick> {
        #[cfg(feature = "perf-stats")]
        {
            let snapshot = beam_perf::Perf::snapshot();
            // 一个样本都没有 ⇒ 还没有真实数据 ⇒ 不发事件（不编造）。
            if snapshot.samples == 0 {
                return None;
            }
            // E2E 阶段耗时：采集器里若没有这一项，就是没有数据（留 0 而不是编造）。
            let e2e_ms = snapshot
                .stages
                .iter()
                .find(|(stage, _)| stage.as_str() == "e2e")
                .map(|(_, s)| s.mean_ns as f32 / 1_000_000.0)
                .unwrap_or(0.0);
            Some(PerfTick {
                e2e_ms,
                fps: snapshot.fps,
                bitrate_mbps: snapshot.bitrate_bps as f32 / 1_000_000.0,
                cpu_pct: snapshot.cpu_percent().unwrap_or(0.0) as f32,
                gpu_pct: snapshot.gpu_percent().unwrap_or(0.0) as f32,
                memory_mb: snapshot
                    .memory_used_bytes()
                    .map(|b| (b / (1024 * 1024)) as u32)
                    .unwrap_or(0),
                timestamp_ms: now_ms(),
            })
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            None
        }
    }

    fn perf_export(&self, format: &str) -> Result<PerfExportResult, BeamError> {
        #[cfg(feature = "perf-stats")]
        {
            let fmt = match format.to_ascii_lowercase().as_str() {
                "csv" => beam_perf::ExportFormat::Csv,
                "json" => beam_perf::ExportFormat::Json,
                other => {
                    return Err(BeamError::invalid(
                        E_PERF_UNAVAILABLE,
                        format!("不支持的导出格式 {other}（只支持 csv / json）"),
                    ))
                }
            };
            let mut path = std::env::temp_dir();
            path.push(format!("beam-perf-export.{format}"));
            let rows = beam_perf::Perf::snapshot().samples as u32;
            beam_perf::Perf::export(fmt, &path).map_err(|err| {
                BeamError::new(
                    E_PERF_UNAVAILABLE,
                    format!("性能数据导出失败: {err}"),
                    false,
                )
            })?;
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Ok(PerfExportResult {
                format: format.to_string(),
                path: path.to_string_lossy().to_string(),
                bytes,
                rows,
            })
        }
        #[cfg(not(feature = "perf-stats"))]
        {
            // §3.13：perf-stats 是 beam-perf 的 cargo feature,桌面端默认不引入,
            // 所以这里明确返回「不可用」,而不是编造数据。
            Err(BeamError::invalid(
                E_PERF_UNAVAILABLE,
                format!("性能统计模块(beam-perf / perf-stats)未接入桌面端,无法导出 {format}"),
            ))
        }
    }

    fn rotate_pairing_code_if_due(&self) -> Option<PairingCode> {
        let mut inner = self.lock();
        let elapsed = inner.pairing.rotated_at_ms;
        if now_ms().saturating_sub(elapsed) < PIN_TTL.as_millis() as u64 {
            return None;
        }
        let (code, token) = self.build_pairing(&inner);
        inner.pairing = code.clone();
        // §3.3：token 跟着 PIN 一起轮换（两者同属一份配对码）。
        inner.host_pairing_token = Some(token);
        inner.host.revision += 1;
        Some(code)
    }

    fn set_host_error(&self, message: String) {
        let mut inner = self.lock();
        inner.host.state = HostState::Error;
        inner.host.message = Some(message);
        inner.host.revision += 1;
        inner.host.updated_at_ms = now_ms();
    }

    fn take_transport_outcome(&self) -> Option<TransportOutcome> {
        self.transport_outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    fn take_session_events(&self) -> Vec<SessionEventInfo> {
        let mut events: Vec<SessionEventInfo> = self
            .session_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        if events.is_empty() {
            return events;
        }
        // 会话状态同步到桌面端状态机(§3.9)。
        {
            let mut inner = self.lock();
            // 「等待主机确认」只提示一次（客户端每秒重试一次,不能每次都弹）。
            let mut waiting_notice = false;
            for event in &events {
                match event {
                    SessionEventInfo::Negotiated {
                        width,
                        height,
                        fps,
                        codec,
                    } => {
                        let _ = inner.state.transition(SessionState::Streaming);
                        if let Some(session) = inner.session.as_mut() {
                            session.state = "streaming".to_string();
                            session.resolution = resolution_label(*width, *height);
                            session.fps = *fps;
                            session.codec = codec.clone();
                        }
                    }
                    // §3.3「还没配对好」（E3008）：**不是**错误 ——
                    // 用户在主机那边点「同意」之前，客户端会一直重试（§4.1）。
                    // 这里把它写成「等待确认」，**不**把会话判死。
                    SessionEventInfo::Error { code, .. }
                        if *code == crate::service::PAIRING_PENDING_CODE =>
                    {
                        if let Some(session) = inner.session.as_mut() {
                            session.state = "waiting_approval".to_string();
                        }
                        waiting_notice = true;
                    }
                    SessionEventInfo::Error { .. } | SessionEventInfo::Closed => {
                        let _ = inner.state.transition(SessionState::Error);
                        if let Some(session) = inner.session.as_mut() {
                            session.state = "error".to_string();
                        }
                    }
                    SessionEventInfo::WaitingApproval => {}
                    SessionEventInfo::Paired { .. } | SessionEventInfo::Stats { .. } => {}
                }
            }
            inner.host.revision += 1;
            inner.host.updated_at_ms = now_ms();
            if waiting_notice && !inner.waiting_approval_notice {
                inner.waiting_approval_notice = true;
                events.push(SessionEventInfo::WaitingApproval);
            }
        }
        events
    }

    fn take_new_pair_requests(&self) -> Vec<PairRequest> {
        // 发现到的主机 → 待确认的配对请求(§3.3)。
        self.collect_pair_candidates();
        let mut inner = self.lock();
        std::mem::take(&mut inner.new_pairs)
    }
}

impl CrateService {
    fn save_trust(&self, inner: &CrateInner) -> Result<(), BeamError> {
        let Some(path) = inner.trust_path.clone() else {
            return Ok(());
        };
        inner
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .save(&path)
            .map_err(|err| {
                BeamError::new(
                    &err.code_str(),
                    format!("写入信任库失败 {}: {err}", err.code_str()),
                    false,
                )
            })
    }

    /// 在后台建立**真实的客户端会话**(§3.9)。
    ///
    /// 流程:
    /// 1. 真实 QUIC/TLS 握手(§3.2)——证书由信任库校验(§3.3);
    /// 2. 客户端传输打开控制双向流与输入单向流(§5);
    /// 3. `beam-client` 组装客户端会话:能力协商、帧回调、输入回传;
    /// 4. 循环推进会话,把会话事件回报给事件层。
    ///
    /// **不阻塞调用方**:`connect` 是同步 command(跑在 UI 线程上),
    /// 在这里等握手会把界面卡住最多 `CONNECT_TIMEOUT`。
    fn spawn_client_session(
        &self,
        resolved: ResolvedConfig,
        peer: PeerAddr,
        host_name: String,
        identity: Identity,
        trust: TrustStore,
        opts: ClientSessionOptions,
    ) {
        *self
            .transport_outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;

        let addr = format!("{}:{}", peer.host, peer.port)
            .parse::<SocketAddr>()
            .unwrap_or_else(|_| SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 47990));

        // 后台任务需要拿到共享状态:用 Arc 克隆,不借用 `self`(否则 future 不满足 'static)。
        let outcome_slot = self.transport_outcome.clone();
        let inner = self.inner.clone();
        let events_slot = self.session_events.clone();

        // QUIC 需要 tokio reactor:在 tauri 的异步运行时里跑,
        // 不用 futures::executor::block_on(它不驱动 reactor,会挂住)。
        tauri::async_runtime::spawn(async move {
            // §3.3：本机指纹（放进挑战载荷，让主机知道客户端是谁）。
            // 先取出来 —— `identity` 下面要移交给连接请求。
            let client_fingerprint = identity.fingerprint();
            // §4.1「新连接/迁移」：把**可重建**的传输工厂交给会话层。
            //
            // 旧做法是「连接在工厂之外先建好」+ 一次性工厂 ⇒ 断线后换不了连接，
            // 「用户在主机上点同意后自动连上」在产品里也就不会发生。
            // 现在工厂自己知道**怎么建**连接：地址、网络参数、校验器、运行时。
            let net_cfg = crate::transport::net_config(&resolved, &identity);
            // §3.3：证书校验交给信任库（网络层不理解配对语义）。
            let verifier: Arc<dyn beam_pair::PeerVerifier> = Arc::new(trust);
            // §3.7 音频：数据报读点按帧头 `type` 分流 —— 音频走「Opus 解码 +
            // WASAPI 输出」，画面走画面重组器。**用 beam-client 的分流传输器**，
            // 桌面端不需要直接依赖 beam-audio（§4 分层）。
            let audio_cfg = beam_client::AudioConfig {
                mode: match resolved.audio.mode {
                    beam_config::AudioMode::HostAndStream => beam_client::AudioMode::HostAndStream,
                    beam_config::AudioMode::StreamOnly => beam_client::AudioMode::StreamOnly,
                    beam_config::AudioMode::Off => beam_client::AudioMode::Off,
                },
                bitrate_bps: resolved.audio.bitrate_bps,
                sample_rate: beam_client::AudioConfig::default().sample_rate,
                channels: beam_client::AudioConfig::default().channels,
            };
            let factory =
                beam_client::DemuxTransportFactory::with_audio(addr, net_cfg, verifier, audio_cfg)
                    // 建链在**当前运行时**里异步进行（§4.1）：会话层只发起、不阻塞。
                    .with_runtime(tokio::runtime::Handle::current());
            // 帧通道统计句柄：上层据此知道有多少帧被丢弃（E2004）；跨重连有效。
            let frame_stats = factory.frame_stats_handle();

            let runtime = ClientRuntime::new({
                // §3.3：把用户输入的 PIN 与**本机**指纹交给客户端运行时 ——
                // PIN 用于发起 SPAKE2，本机指纹放进挑战载荷（让主机知道是谁）。
                let mut cfg = ClientConfig::from_resolved(&resolved);
                cfg.pin = opts.pin.clone();
                cfg.fingerprint = Some(client_fingerprint);
                cfg
            });

            // §3.8：开关打开时装上客户端键鼠采集（低级钩子）。
            //
            // 采集失败**不影响连接**：只是「本次不回传输入」，画面照常。
            let runtime = if opts.share_input {
                match beam_client::capture::WindowsInputCapture::start() {
                    Ok(capture) => {
                        if capture.is_installed() {
                            tracing::info!("已启用键鼠回传：低级钩子采集已启动");
                        } else {
                            tracing::warn!(
                                reason = %capture.error().unwrap_or_default(),
                                "键鼠采集钩子未能安装，本次不回传输入"
                            );
                        }
                        runtime.with_input(Arc::new(capture))
                    }
                    Err(e) => {
                        tracing::warn!(reason = %e, "键鼠采集启动失败，本次不回传输入");
                        runtime
                    }
                }
            } else {
                runtime
            };

            // §3.10 / 产品方案第 45 行:客户端画面上屏到**独立原生窗口**
            // (不走 WebView)。分工:
            // - L3(`beam-client`)建窗口 + 组装「硬解 + D3D11 flip-model 交换链」;
            // - 外壳(L4,本文件)不碰 Win32 / D3D11(§4 分层)。
            #[cfg(feature = "client-video")]
            let title = format!("Beam 串流 - {host_name}");
            #[cfg(feature = "client-video")]
            let session = match runtime.connect_to_window(
                peer,
                &title,
                u32::from(resolved.video.width),
                u32::from(resolved.video.height),
                Some(Box::new(factory)),
            ) {
                Ok(session) => Some(session),
                Err(err) => {
                    report_transport_failure(
                        &inner,
                        &outcome_slot,
                        &BeamError::new(E_PRESENT_FAILED, err.to_string(), false),
                        &host_name,
                    );
                    return;
                }
            };

            // `session`（含窗口）必须活到循环结束 ⇒ 它比 `handle` 后声明,
            // 因此在作用域结束时**先**释放会话、**后**释放窗口(顺序正确)。
            #[cfg(feature = "client-video")]
            let handle = match session.as_ref() {
                Some(session) => session.handle.clone(),
                None => return,
            };

            // 没开 `client-video` feature 时:**如实**说明上屏不可用,
            // 而不是让会话看起来「在串流」却一帧都不出。
            #[cfg(not(feature = "client-video"))]
            let handle = {
                let handle = runtime.connect_default(peer, Some(Box::new(factory)));
                tracing::warn!(
                    "未启用 `client-video` feature:解码/上屏后端不可用(E10014),本次会话不会有画面"
                );
                handle
            };
            {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.client_session = Some(handle.clone());
                guard.frame_stats = Some(frame_stats);
            }

            // 推进会话:取控制消息 / 取帧 / 回报统计。
            let mut events = handle.events();
            loop {
                let _ = handle.pump();
                while let Some(Some(event)) = events.next().now_or_never() {
                    match event {
                        // §3.3「还没配对好」（E3008）**不是断开**：
                        // 用户在主机那边点「同意」之前，客户端会按 §4.1 持续重试。
                        // 事件层看到这个码不会发「断开」（只记日志）。
                        SessionEvent::Error { code, retryable }
                            if code == crate::service::PAIRING_PENDING_CODE =>
                        {
                            events_slot
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(SessionEventInfo::Error { code, retryable });
                        }
                        // 致命错误:交给「传输结果」通道上报（事件层只发一次断开事件）。
                        SessionEvent::Error { code, .. } => {
                            *outcome_slot.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(TransportOutcome {
                                    ok: false,
                                    code: Some(format!("E{code}")),
                                    message: format!("客户端会话出错(E{code})"),
                                    host_name: host_name.clone(),
                                });
                            {
                                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                                let _ = guard.state.transition(SessionState::Error);
                                if let Some(session) = guard.session.as_mut() {
                                    session.state = "error".to_string();
                                }
                            }
                        }
                        other => {
                            if let Some(info) = translate_session_event(other) {
                                events_slot
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push(info);
                            }
                        }
                    }
                }
                if handle.is_stopped() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
    }

    /// 起**主机侧** QUIC 监听,并在后台接受客户端接入(§3.2 / §3.9 / 产品方案 §8)。
    ///
    /// 这一段补的是「桌面端作为主机」这一侧:
    ///
    /// 1. 用**共享信任库**构造信任闸门([`HostTrustGate`])——
    ///    未写入信任库的对端**一律拒绝握手**,但它的完整指纹被记住(§3.3);
    /// 2. 起 QUIC 监听(§3.2);
    /// 3. 循环接受连接:
    ///    - **握手被拒** → 闸门已记下指纹 → 生成「待确认的配对请求」
    ///      → 前端弹窗让用户确认(产品方案 §8 不可省略的安全环节)
    ///      → 用户点「同意」写信任库 → **下一次握手**才放行;
    ///    - **握手成功** → 起主机会话(§3.9),并在后台 `pump()`,
    ///      真实错误(不是 `E6005`「本轮暂无输出」)从这里冒泡到主机状态。
    ///
    /// **不阻塞调用方**:`start_hosting` 是同步 command(跑在 UI 线程上),
    /// 建 QUIC 端点必须异步,所以监听在后台起,结果写回主机状态。
    fn spawn_host_listener(
        &self,
        resolved: ResolvedConfig,
        identity: Identity,
        trust: Arc<Mutex<TrustStore>>,
        bind_ip: Ipv4Addr,
        port: u16,
    ) {
        let inner = self.inner.clone();
        // 闸门与桌面端共享同一份信任库:用户点「同意」写库后,闸门立刻能看到。
        let gate = Arc::new(HostTrustGate::new(trust));
        // §3.3:**配对模式** —— 未信任对端的握手也放行。
        //
        // 为什么:客户端要把 PIN 交给主机校验,只能走**控制通道**;
        // 如果握手就被拒,控制通道根本开不了,PIN 也就没有用武之地。
        // 安全兜底移到控制面:主机在 `require_pairing` 为真时
        // **拒绝**未配对对端的 `Hello`(beam-session 的控制层把关),
        // 所以「握手放行」不等于「能串流」。
        //
        // 未信任的对端照样被记进 `observed` ⇒ 用户在弹窗里点「同意」
        // 这条路(产品方案 §8)仍然保留。
        gate.set_pairing_mode(true);

        tauri::async_runtime::spawn(async move {
            let request = crate::transport::ListenRequest {
                resolved: resolved.clone(),
                identity: identity.clone(),
                bind_addr: SocketAddr::new(bind_ip.into(), port),
            };
            // §3.3:证书校验交给闸门 —— 未信任的对端在这里就被挡住。
            let verifier: Arc<dyn PeerVerifier> = Arc::clone(&gate) as Arc<dyn PeerVerifier>;
            let listener = match crate::transport::listen_peer(request, verifier).await {
                Ok(listener) => listener,
                Err(err) => {
                    // 监听失败 = 开播失败:如实反映到主机状态,不假装在播。
                    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.host.state = HostState::Error;
                    guard.host.message = Some(format!("QUIC 监听失败: {}", err.message));
                    guard.host.revision += 1;
                    guard.host.updated_at_ms = now_ms();
                    tracing::error!(reason = %err.message, "主机 QUIC 监听失败");
                    return;
                }
            };
            let listener = Arc::new(listener);
            {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.host_listener = Some(Arc::clone(&listener));
                guard.host_gate = Some(Arc::clone(&gate));
                guard.host.message = None;
                guard.host.revision += 1;
                guard.host.updated_at_ms = now_ms();
            }

            // 接受循环。
            loop {
                match listener.accept().await {
                    Ok(conn) => {
                        // §3.3：握手通过 ⇒ 取走这次握手的**信任判定**。
                        // - `Trusted` ⇒ 对端已在信任库里，不需要再配对；
                        // - `NeedsPairing` ⇒ 对端是新的，控制面必须先完成
                        //   PIN / token 交换（§4.1）。
                        let verdict = gate.take_verdict();
                        // 主机当前显示的 PIN（每 `pin_ttl=120s` 轮换一次，
                        // 所以**每次接受连接时现取**，不用构造期缓存的）。
                        let (pin, token, host_fp) = {
                            let g = inner.lock().unwrap_or_else(|e| e.into_inner());
                            (
                                Some(g.pairing.code.clone()),
                                g.host_pairing_token.clone(),
                                Some(g.identity.fingerprint()),
                            )
                        };
                        let pairing =
                            beam_host::HostPairing::for_verdict(verdict, pin, token, host_fp);
                        tracing::info!(
                            require_pairing = pairing.require_pairing,
                            "客户端已通过信任校验,建立主机会话"
                        );
                        spawn_host_session(inner.clone(), resolved.clone(), conn, pairing);
                    }
                    Err(err) => {
                        if listener.is_closed() {
                            break;
                        }
                        // 握手被拒是**预期**的:未信任的对端会反复重连,
                        // 每次都产生/刷新一条待确认的配对请求(§3.3)。
                        collect_host_pair_requests(&inner, &gate);
                        tracing::debug!(reason = %err, "接受连接失败(可能是对端尚未获得信任)");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        });
    }

    /// 把发现到的、尚未信任的主机变成「待确认的配对请求」(§3.3 / 产品方案 §8)。
    ///
    /// 产品方案第 102 行把「人工确认后才写入信任库」列为不可省略的安全环节:
    /// 这里产生的只是**待确认**请求,用户点「同意」才写库。
    ///
    /// §3.4 的 mDNS 只广播指纹前 8 位,而信任库需要完整指纹(§3.3),
    /// 因此这里先在后台做一次指纹采集(握手必然失败,只取对端证书里的指纹),
    /// 采集到并且与 mDNS 广播的前缀一致,才生成真正的待确认请求。
    fn collect_pair_candidates(&self) {
        let (hosts, resolved, identity) = {
            let inner = self.lock();
            (
                self.discovery.snapshot(),
                inner.resolved.clone(),
                inner.identity.clone(),
            )
        };
        let trusted: Vec<String> = self
            .lock()
            .trust
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peers()
            .iter()
            .map(|p| p.fp.short_hex().to_lowercase())
            .collect();

        for host in hosts {
            let prefix = host.fingerprint.to_lowercase();
            if prefix.is_empty() || trusted.contains(&prefix) {
                continue;
            }
            {
                let inner = self.lock();
                if inner
                    .pending_pairs
                    .iter()
                    .any(|r| r.id.starts_with(&prefix))
                    || inner.rejected_pairs.contains(&prefix)
                    || inner.probing.contains(&host.id)
                {
                    continue;
                }
            }
            // 标记为正在采集,避免重复发起。
            self.lock().probing.push(host.id.clone());
            self.spawn_fingerprint_probe(host, prefix, resolved.clone(), identity.clone());
        }
    }

    /// 后台采集某台主机的完整证书指纹,并据此生成待确认的配对请求。
    fn spawn_fingerprint_probe(
        &self,
        host: beam_discovery::DiscoveredHost,
        expected_prefix: String,
        resolved: ResolvedConfig,
        identity: Identity,
    ) {
        let inner = self.inner.clone();
        tauri::async_runtime::spawn(async move {
            let request = crate::transport::ProbeRequest {
                addr: SocketAddr::new(std::net::IpAddr::V4(host.address), host.port),
                resolved,
                identity,
            };
            let fp = match crate::transport::probe_fingerprint(request).await {
                Ok(fp) => fp,
                Err(err) => {
                    tracing::warn!(
                        code = %err.code,
                        host = %host.name,
                        "采集主机证书指纹失败,无法生成配对请求"
                    );
                    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.probing.retain(|id| id != &host.id);
                    return;
                }
            };
            if fp.short_hex().to_lowercase() != expected_prefix {
                // mDNS 广播的指纹与证书对不上:不生成请求(宁可少一条,不能错一条)。
                tracing::warn!(
                    host = %host.name,
                    expected = %expected_prefix,
                    actual = %fp.short_hex(),
                    "mDNS 指纹与证书指纹不一致,放弃生成配对请求"
                );
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.probing.retain(|id| id != &host.id);
                return;
            }
            let request = PairRequest {
                id: fp.to_hex(),
                name: host.name.clone(),
                platform: PLATFORM_UNKNOWN.to_string(),
                address: host.address.to_string(),
                fingerprint: fp.to_hex(),
                requested_at_ms: now_ms(),
            };
            let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.probing.retain(|id| id != &host.id);
            guard.pending_pairs.push(request.clone());
            guard.new_pairs.push(request);
        });
    }
}

/// 建链失败:把结果回报给事件层,并把会话置为错误态。
fn report_transport_failure(
    inner: &Arc<Mutex<CrateInner>>,
    outcome_slot: &Arc<Mutex<Option<TransportOutcome>>>,
    err: &BeamError,
    host_name: &str,
) {
    {
        let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
        let _ = guard.state.transition(SessionState::Error);
        if let Some(session) = guard.session.as_mut() {
            session.state = "error".to_string();
        }
    }
    *outcome_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(TransportOutcome {
        ok: false,
        code: Some(err.code.clone()),
        message: err.message.clone(),
        host_name: host_name.to_string(),
    });
}

/// 起主机会话并在后台推进(§3.9)。
///
/// 画面流水线由 `beam-host` 按 feature 决定(§4:桌面端**不能**直接依赖平台 crate):
/// 开了 `platform` 就是 WGC 抓屏 + NVENC 硬编的真后端,没开就是空流水线
/// (控制面照常,**一帧画面都不产出**)。
///
/// 错误处理:**真实错误**从 [`beam_host::HostSessionHandle::pump`] 冒泡,
/// 直接反映到主机状态 —— 不用 `.ok()` 吞掉。
/// 「编码器尚未产出帧」(`E6005`) 在 `beam-host` 侧已被显式容忍,
/// 不会出现在这里(见决策 D6)。
fn spawn_host_session(
    inner: Arc<Mutex<CrateInner>>,
    resolved: ResolvedConfig,
    conn: beam_net::Conn,
    pairing: beam_host::HostPairing,
) {
    tauri::async_runtime::spawn(async move {
        let runtime = HostRuntime::new(HostConfig::from_resolved(&resolved));
        let (pipeline, errors) = beam_host::default_frame_pipeline(runtime.config());
        // 输入:桌面端没有平台注入器(§4 禁止直接依赖 beam-input),
        // 用 beam-host 提供的空接收器把输入流读走 —— 客户端的输入通道不会堵住。
        let input = beam_host::NoopInputSink::new();
        let handle = match runtime
            .start_quic_session_with_pipeline(conn, pipeline, errors, Some(input), pairing)
            .await
        {
            Ok(handle) => Arc::new(handle),
            Err(err) => {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.host.state = HostState::Error;
                guard.host.message = Some(format!("主机会话建立失败: {err}"));
                guard.host.revision += 1;
                guard.host.updated_at_ms = now_ms();
                tracing::error!(reason = %err, "主机会话建立失败");
                return;
            }
        };
        {
            let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.host_session = Some(Arc::clone(&handle));
            guard.host.state = HostState::Streaming;
            // 对端此时还没在控制面上自我介绍(它的设备名/平台要等能力协商),
            // 所以这里**如实**写成「未知设备 / 未知平台」,不编造。
            guard.host.connected_device = Some(Device {
                id: "unknown".to_string(),
                name: "未知设备".to_string(),
                platform: PLATFORM_UNKNOWN.to_string(),
                fingerprint: String::new(),
                paired_at: String::new(),
                online: true,
                streaming: true,
                last_session: None,
                perms: DevicePerms {
                    view: true,
                    keyboard: true,
                    mouse: true,
                    gamepad: true,
                },
            });
            guard.host.message = None;
            guard.host.revision += 1;
            guard.host.updated_at_ms = now_ms();
        }

        // §3.3：配对成功 ⇒ 把对端指纹**真的写进信任库**。
        //
        // 这一步放在 `beam-host` 里(主机编排层owns 主机侧的信任语义)，
        // 桌面端与真机验证入口共用**同一份实现**。
        //
        // PIN 路径不需要用户再点一次「同意」:用户在客户端输入了主机屏幕上
        // 显示的 PIN,这本身就是授权。写库之后,这个对端下次握手就是「已信任」
        // ⇒ 不用再配对(§3.3「已配对的主机可传空串」的对称面)。
        {
            let trust = {
                let guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                Arc::clone(&guard.trust)
            };
            handle.spawn_trust_writer(trust);
        }

        // 推进会话:真实错误从这里冒泡(不静默吞掉)。
        loop {
            if let Err(err) = handle.pump() {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.host.state = HostState::Error;
                guard.host.message = Some(format!("串流出错: {err}"));
                guard.host.revision += 1;
                guard.host.updated_at_ms = now_ms();
                tracing::error!(reason = %err, "主机会话出错");
                return;
            }
            if handle.session.is_stopped() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
}

/// 把闸门观察到的「未信任对端」变成**待确认的配对请求**(§3.3 / 产品方案 §8)。
///
/// 这是**主机侧**的人工确认入口:
///
/// - 闸门在握手时拒绝了未信任的对端,并记下了它的**完整**指纹;
/// - 这里把观察记录转成配对请求 → `pair://request` 事件 → 前端弹窗;
/// - 用户点「同意」→ `approve_pair` 写信任库 → **下一次**握手放行。
///
/// 请求 id 用**完整指纹**(与客户端侧一致),这样 `approve_pair` 的
/// `Fingerprint::from_hex` 能直接解析。
///
/// 设备名与平台:对端还没有连上控制面,拿不到它的自我介绍,
/// 因此这里**如实**写成「未知设备 / 未知平台」,不编造。
fn collect_host_pair_requests(inner: &Arc<Mutex<CrateInner>>, gate: &Arc<HostTrustGate>) {
    let observed = gate.observed();
    if observed.is_empty() {
        return;
    }
    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
    for peer in observed {
        let id = peer.fingerprint_hex();
        if guard.pending_pairs.iter().any(|r| r.id == id) {
            // 已经在待确认列表里:更新尝试次数即可,不重复生成。
            continue;
        }
        let request = PairRequest {
            id: id.clone(),
            name: "未知设备".to_string(),
            platform: PLATFORM_UNKNOWN.to_string(),
            address: String::new(),
            fingerprint: id,
            requested_at_ms: now_ms(),
        };
        guard.pending_pairs.push(request.clone());
        guard.new_pairs.push(request);
    }
}

/// 把 `beam-session` 的事件翻译成桌面端这一层的事件。
fn translate_session_event(event: SessionEvent) -> Option<SessionEventInfo> {
    match event {
        SessionEvent::Negotiated { config } => Some(SessionEventInfo::Negotiated {
            width: config.width,
            height: config.height,
            fps: config.fps,
            codec: codec_name(config.codec),
        }),
        SessionEvent::Paired { fingerprint } => Some(SessionEventInfo::Paired { fingerprint }),
        SessionEvent::Stats(report) => Some(SessionEventInfo::Stats {
            fps: report.fps,
            bitrate_bps: report.bitrate_bps,
        }),
        SessionEvent::Error { code, retryable } => {
            Some(SessionEventInfo::Error { code, retryable })
        }
        SessionEvent::Closed => Some(SessionEventInfo::Closed),
        // §3.2 E2004:丢帧**不是**会话错误,既不结束会话也不进 Error 态。
        // 桌面端的丢帧计数走 `frame_stats`(在 `session_stats` 里读),
        // 所以这里不产生会话级事件。
        SessionEvent::FramesDropped { count, code } => {
            tracing::debug!(count, code, "传输层丢弃了过期帧");
            None
        }
        SessionEvent::StateChanged { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// 生成配对码与真实二维码载荷(§3.3)。
///
/// 载荷格式固定为 `beam://pair?h=ip:port&fp=..&t=token`(§3.3),
/// 里面装的是**真实**的本机地址、真实指纹与一次性 token。
fn build_pairing(identity: &Identity, local_ip: Option<&str>, port: u16) -> (PairingCode, String) {
    let host = local_ip.unwrap_or("127.0.0.1");
    let ip: Ipv4Addr = host.parse().unwrap_or(Ipv4Addr::LOCALHOST);
    let addr = SocketAddr::new(ip.into(), port);

    // 真实的一次性 token(§3.3 `pin_ttl`),带真实 TTL 校验。
    let token = PairingToken::issue(PIN_TTL);
    let token_value = token.value().to_string();
    let payload = PairPayload {
        host: addr.ip().to_string(),
        port: addr.port(),
        fingerprint: identity.fingerprint(),
        token: token_value.clone(),
    };

    // 4 位 PIN(§3.3 `pin_len=4`):用随机字节取模,保证每次不同。
    let pin_bytes = beam_pair::generate_token();
    let code: String = pin_bytes
        .as_bytes()
        .iter()
        .take(4)
        .map(|b| char::from(b'0' + (b % 10)))
        .collect();

    (
        PairingCode {
            code,
            qr_payload: payload.to_uri(),
            expires_in_s: PIN_TTL.as_secs() as u32,
            rotated_at_ms: now_ms(),
        },
        // token 一并返回:主机把它放进会话层的 `PairingConfig.token`
        // ⇒ 扫码那条路走 `PairRequest{token}`(§5),不需要用户手输 PIN。
        token_value,
    )
}

/// `beam_proto::CodecKind` → 前端使用的 codec 名。
///
/// §3.14 的配置口径与前端 `SettingsPage` 一致,都是小写
/// (`hevc` / `h264` / `av1`)。
fn codec_name(kind: CodecKind) -> String {
    match kind {
        CodecKind::H264 => "h264".to_string(),
        CodecKind::Hevc => "hevc".to_string(),
        CodecKind::Av1 => "av1".to_string(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "BEAM-PC".to_string())
}

fn detect_local_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let addr = sock.local_addr().ok()?;
    if addr.ip().is_unspecified() {
        None
    } else {
        Some(addr.ip().to_string())
    }
}

fn resolution_label(width: u16, height: u16) -> String {
    match (width, height) {
        (1280, 720) => "720p".to_string(),
        (1920, 1080) => "1080p".to_string(),
        (2560, 1440) => "1440p".to_string(),
        (3840, 2160) => "4k".to_string(),
        _ => format!("{width}x{height}"),
    }
}

fn resolution_dims(label: &str) -> Option<(u16, u16)> {
    match label {
        "720p" => Some((1280, 720)),
        "1080p" => Some((1920, 1080)),
        "1440p" => Some((2560, 1440)),
        "4k" => Some((3840, 2160)),
        _ => None,
    }
}

fn settings_from(resolved: &ResolvedConfig, ui: &DesktopSettings) -> Settings {
    Settings {
        resolution: resolution_label(resolved.video.width, resolved.video.height),
        fps: resolved.video.fps,
        bitrate_mbps: resolved.video.bitrate_bps / 1_000_000,
        hdr: resolved.video.hdr,
        match_client_resolution: ui.match_client_resolution,
        encoder: resolved.video.encoder.clone(),
        codec: resolved.video.codec.clone(),
        preset: ui.preset.clone(),
        // 规范配置项(§3.5 / §3.6):真值在 beam-config,桌面端只做展示。
        capture_backend: resolved.capture.backend.clone(),
        slices: resolved.video.slices.unwrap_or(0),
        sfe: resolved.video.sfe,
        fec_ratio: resolved.network.fec_ratio,
        port: ui.port,
        allow_tailscale: ui.allow_tailscale,
        discovery_enabled: resolved.network.discovery_enabled,
        bitrate_min_mbps: resolved.network.bitrate_min / 1_000_000,
        bitrate_max_mbps: resolved.network.bitrate_max / 1_000_000,
        audio_mode: match resolved.audio.mode {
            AudioMode::HostAndStream => "host+stream".to_string(),
            AudioMode::StreamOnly => "stream_only".to_string(),
            AudioMode::Off => "off".to_string(),
        },
        audio_bitrate_kbps: resolved.audio.bitrate_bps / 1000,
        language: ui.language.clone(),
        theme: ui.theme.clone(),
        show_stats_overlay: ui.show_stats_overlay,
        minimize_to_tray: ui.minimize_to_tray,
        launch_on_startup: ui.launch_on_startup,
        share_input: ui.share_input,
    }
}

fn device_from_peer(peer: &TrustedPeer) -> Device {
    Device {
        id: peer.fp.to_hex(),
        name: peer.name.clone(),
        platform: peer.platform.clone(),
        fingerprint: peer.fp.short_hex(),
        paired_at: format_date(peer.paired_at * 1000),
        online: false,
        streaming: false,
        last_session: None,
        perms: DevicePerms {
            view: peer.perms.view,
            keyboard: peer.perms.input,
            mouse: peer.perms.input,
            gamepad: peer.perms.gamepad,
        },
    }
}

/// 启动真实 mDNS 浏览(§3.4)。
fn spawn_discovery_browser(state: Arc<DiscoveryState>) {
    let browser = match Browser::start() {
        Ok(browser) => browser,
        Err(err) => {
            tracing::warn!(code = %err.code_str(), "mDNS 浏览启动失败,局域网列表将为空");
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("beam-discovery".to_string())
        .spawn(move || {
            let mut browser = browser;
            loop {
                // mDNS 事件流不依赖 tokio reactor,可以在独立线程里阻塞等待。
                match futures::executor::block_on(browser.next()) {
                    Some(DiscoveryEvent::Found(host)) => state.upsert(host),
                    Some(DiscoveryEvent::Lost { fullname }) => state.remove(&fullname),
                    None => break,
                }
            }
        });
    if spawned.is_err() {
        tracing::warn!("无法创建 mDNS 浏览线程");
    }
}

fn format_date(ms: u64) -> String {
    let days = ((ms / 1000) as i64).div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
