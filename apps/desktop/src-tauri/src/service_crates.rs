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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use beam_client::ClientConfig;
use beam_config::{
    default_config_path, default_trust_path, load_file_layer, merge, save_file_layer, validate,
    AudioMode, ConfigLayers, ResolvedConfig, TomlFileStore,
};
use beam_discovery::{Advertiser, Browser, DiscoveryEvent, HostInfo};
use beam_host::HostConfig;
use beam_pair::{
    default_identity_path, FileKeyStore, Fingerprint, Identity, PairPayload, PairingToken,
    PeerPerms, TrustStore, TrustedPeer,
};
use beam_proto::CodecKind;
use beam_session::{PeerAddr, SessionState, StateMachine};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::dto::*;
use crate::error::*;
use crate::service::{BeamService, TransportOutcome};

/// §3.3 `pin_ttl=120s`。
const PIN_TTL: Duration = Duration::from_secs(120);

/// 桌面端自有、beam-config 未建模的设置项。
///
/// 持久化在 `%APPDATA%/Beam/desktop.json`;
/// beam-config 负责 §3.14 的通用配置面(画质 / 网络 / 输入 / 音频)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSettings {
    pub match_client_resolution: bool,
    pub preset: String,
    pub capture_backend: String,
    pub slices: u32,
    pub sfe: bool,
    pub port: u16,
    pub allow_tailscale: bool,
    pub language: String,
    pub theme: String,
    pub show_stats_overlay: bool,
    pub minimize_to_tray: bool,
    pub launch_on_startup: bool,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            match_client_resolution: true,
            preset: "low_latency_p1".to_string(),
            capture_backend: "auto".to_string(),
            slices: 1,
            sfe: false,
            port: 47990,
            allow_tailscale: false,
            language: "zh-CN".to_string(),
            theme: "dark".to_string(),
            show_stats_overlay: true,
            minimize_to_tray: true,
            launch_on_startup: false,
        }
    }
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
    trust: TrustStore,
    state: StateMachine,
    advertiser: Option<Advertiser>,
    session: Option<SessionInfo>,
    pending_pairs: Vec<PairRequest>,
    /// 当前配对码(含真实二维码载荷与轮换时间,§3.3)。
    pairing: PairingCode,
    config_path: Option<PathBuf>,
    trust_path: Option<PathBuf>,
    desktop_path: Option<PathBuf>,
}

/// crates/ 适配器。
pub struct CrateService {
    inner: Arc<Mutex<CrateInner>>,
    discovery: Arc<DiscoveryState>,
    /// 传输层握手结果槽:后台任务写入,事件层取走。
    transport_outcome: Arc<Mutex<Option<TransportOutcome>>>,
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

        // §3.3:信任库。
        let trust = match &trust_path {
            Some(path) => TrustStore::load(path).unwrap_or_else(|err| {
                tracing::warn!(code = %err.code_str(), "读取信任库失败,按空库处理");
                TrustStore::new()
            }),
            None => TrustStore::new(),
        };

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
        let pairing = build_pairing(&identity, host.local_ip.as_deref(), ui.port);

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
                pending_pairs: Vec::new(),
                pairing,
                config_path,
                trust_path,
                desktop_path,
            })),
            discovery,
            transport_outcome: Arc::new(Mutex::new(None)),
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
    fn build_pairing(&self, inner: &CrateInner) -> PairingCode {
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
            BeamError::invalid(
                E_CONFIG_INVALID,
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
                .unwrap_or_else(|| "未知平台".to_string()),
            paired_at: now_ms() / 1000,
            perms: PeerPerms {
                view: true,
                input: true,
                audio: true,
                gamepad: true,
            },
        };
        // 真实写入信任库(§3.3)。
        inner.trust.upsert(peer);
        inner.pending_pairs.retain(|r| r.id != id);
        self.save_trust(&inner)?;
        Ok(device_from_peer(
            inner.trust.peers().last().expect("刚刚 upsert 过"),
        ))
    }

    fn reject_pair(&self, id: &str) -> Result<(), BeamError> {
        let mut inner = self.lock();
        let before = inner.pending_pairs.len();
        inner.pending_pairs.retain(|r| r.id != id);
        if inner.pending_pairs.len() == before {
            return Err(BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("配对请求 {id} 不存在"),
            ));
        }
        Ok(())
    }

    fn remove_device(&self, id: &str) -> Result<(), BeamError> {
        let mut inner = self.lock();
        let fp = Fingerprint::from_hex(id).map_err(|err| {
            BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("指纹非法 {}: {err}", err.code_str()),
            )
        })?;
        if !inner.trust.remove(&fp) {
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
        inner.trust.peers().iter().map(device_from_peer).collect()
    }

    fn pair_requests(&self) -> Vec<PairRequest> {
        self.lock().pending_pairs.clone()
    }

    fn list_hosts(&self) -> Vec<DiscoveredHost> {
        let inner = self.lock();
        let trusted: Vec<String> = inner
            .trust
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
            })
            .collect()
    }

    fn connect(&self, host_id: &str) -> Result<SessionInfo, BeamError> {
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
        let peer = PeerAddr {
            host: host.address.to_string(),
            port: host.port,
            fingerprint: Some(host.fingerprint.clone()),
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
        inner.session = Some(session.clone());

        // 真实 QUIC 握手放到后台跑(§3.2),结果通过事件层回报。
        let identity = inner.identity.clone();
        let trust = inner.trust.clone();
        self.spawn_transport(
            inner.resolved.clone(),
            peer,
            host.name.clone(),
            identity,
            trust,
        );

        Ok(session)
    }

    fn disconnect(&self) -> Result<SessionInfo, BeamError> {
        let mut inner = self.lock();
        let session = inner
            .session
            .take()
            .ok_or_else(|| BeamError::invalid(E_SESSION_INVALID_STATE, "当前没有串流会话"))?;
        let _ = inner.state.transition(SessionState::Closed);
        Ok(session)
    }

    fn session(&self) -> Option<SessionInfo> {
        self.lock().session.clone()
    }

    fn session_stats(&self) -> Option<SessionStats> {
        let inner = self.lock();
        let session = inner.session.clone()?;
        // 真实会话还没跑起来(平台后端是占位实现),这里给的是协商后的配置,
        // 不冒充实测统计:各项耗时字段留 0,由前端按「未采集」显示。
        Some(SessionStats {
            session_id: session.session_id.clone(),
            e2e_ms: 0.0,
            capture_ms: 0.0,
            color_convert_ms: 0.0,
            encode_ms: 0.0,
            network_ms: 0.0,
            decode_ms: 0.0,
            render_ms: 0.0,
            fps: session.fps,
            bitrate_mbps: inner.resolved.video.bitrate_bps as f32 / 1_000_000.0,
            loss_pct: 0.0,
            fec_recovered_pct: 0.0,
            jitter_ms: 0.0,
            dropped_frames: 0,
            resolution: session.resolution.clone(),
            codec: session.codec.clone(),
            timestamp_ms: now_ms(),
        })
    }

    fn reconnect_notice(&self) -> Option<ReconnectingInfo> {
        // 不合成重连通知:只有真实传输层报告断线时才应该产生。
        // 当前 `beam-session` 的帧传输封装尚未落地,这里恒为 `None`。
        None
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
                BeamError::invalid(E_CONFIG_INVALID, format!("不支持的分辨率 {res}"))
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
            BeamError::invalid(
                E_CONFIG_INVALID,
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
        if let Some(v) = patch.capture_backend {
            ui.capture_backend = v;
        }
        if let Some(v) = patch.slices {
            ui.slices = v;
        }
        if let Some(v) = patch.sfe {
            ui.sfe = v;
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

        // 真实落盘:%APPDATA%/Beam/config.toml + desktop.json。
        if let Some(path) = inner.config_path.clone() {
            let store = TomlFileStore::new(path);
            save_file_layer(&store, &layers).map_err(|err| {
                BeamError::new(
                    E_CONFIG_IO,
                    format!("写入配置失败 {}: {err}", err.code_str()),
                    false,
                )
            })?;
        }
        if let Some(path) = inner.desktop_path.clone() {
            let text = serde_json::to_string_pretty(&ui).map_err(|err| {
                BeamError::new(E_CONFIG_IO, format!("序列化桌面设置失败: {err}"), false)
            })?;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, text).map_err(|err| {
                BeamError::new(E_CONFIG_IO, format!("写入桌面设置失败: {err}"), false)
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
        PerfSnapshot {
            available: false,
            stages: Vec::new(),
            metrics: Vec::new(),
            e2e_series: Vec::new(),
            samples: 0,
            captured_at_ms: now_ms(),
        }
    }

    fn perf_tick(&self) -> Option<PerfTick> {
        None
    }

    fn perf_export(&self, format: &str) -> Result<PerfExportResult, BeamError> {
        // §3.13:perf-stats 是 beam-perf 的 cargo feature,桌面端默认不引入,
        // 所以这里明确返回「不可用」,而不是编造数据。
        Err(BeamError::invalid(
            E_PERF_UNAVAILABLE,
            format!("性能统计模块(beam-perf / perf-stats)未接入桌面端,无法导出 {format}"),
        ))
    }

    fn rotate_pairing_code_if_due(&self) -> Option<PairingCode> {
        let mut inner = self.lock();
        let elapsed = inner.pairing.rotated_at_ms;
        if now_ms().saturating_sub(elapsed) < PIN_TTL.as_millis() as u64 {
            return None;
        }
        let code = self.build_pairing(&inner);
        inner.pairing = code.clone();
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
}

impl CrateService {
    fn save_trust(&self, inner: &CrateInner) -> Result<(), BeamError> {
        let Some(path) = inner.trust_path.clone() else {
            return Ok(());
        };
        inner.trust.save(&path).map_err(|err| {
            BeamError::new(
                E_CONFIG_IO,
                format!("写入信任库失败 {}: {err}", err.code_str()),
                false,
            )
        })
    }

    /// 在后台做真实 QUIC 握手(§3.2),结果回报给事件层。
    ///
    /// **不阻塞调用方**:`connect` 是同步 command(跑在 UI 线程上),
    /// 在这里等握手会把界面卡住最多 `CONNECT_TIMEOUT`;
    /// 握手结果改由 `take_transport_outcome` 交给事件层上报。
    fn spawn_transport(
        &self,
        resolved: ResolvedConfig,
        peer: PeerAddr,
        host_name: String,
        identity: Identity,
        trust: TrustStore,
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

        // QUIC 需要 tokio reactor:在 tauri 的异步运行时里跑,
        // 不用 futures::executor::block_on(它不驱动 reactor,会挂住)。
        tauri::async_runtime::spawn(async move {
            let request = crate::transport::TransportRequest {
                addr,
                resolved,
                identity,
                trust,
                server_name: host_name.clone(),
            };
            let outcome = match crate::transport::connect_peer(request).await {
                Ok(()) => TransportOutcome {
                    ok: true,
                    code: None,
                    message: format!("已连接到 {host_name}"),
                    host_name,
                },
                Err(err) => TransportOutcome {
                    ok: false,
                    code: Some(err.code.clone()),
                    message: err.message.clone(),
                    host_name,
                },
            };

            // 握手结果写回会话状态(§3.9)。
            {
                let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                if outcome.ok {
                    let _ = guard.state.transition(SessionState::Streaming);
                    if let Some(session) = guard.session.as_mut() {
                        session.state = "streaming".to_string();
                    }
                } else {
                    let _ = guard.state.transition(SessionState::Error);
                    if let Some(session) = guard.session.as_mut() {
                        session.state = "error".to_string();
                    }
                }
            }

            *outcome_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
        });
    }
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// 生成配对码与真实二维码载荷(§3.3)。
///
/// 载荷格式固定为 `beam://pair?h=ip:port&fp=..&t=token`(§3.3),
/// 里面装的是**真实**的本机地址、真实指纹与一次性 token。
fn build_pairing(identity: &Identity, local_ip: Option<&str>, port: u16) -> PairingCode {
    let host = local_ip.unwrap_or("127.0.0.1");
    let ip: Ipv4Addr = host.parse().unwrap_or(Ipv4Addr::LOCALHOST);
    let addr = SocketAddr::new(ip.into(), port);

    // 真实的一次性 token(§3.3 `pin_ttl`),带真实 TTL 校验。
    let token = PairingToken::issue(PIN_TTL);
    let payload = PairPayload {
        host: addr.ip().to_string(),
        port: addr.port(),
        fingerprint: identity.fingerprint(),
        token: token.value().to_string(),
    };

    // 4 位 PIN(§3.3 `pin_len=4`):用随机字节取模,保证每次不同。
    let pin_bytes = beam_pair::generate_token();
    let code: String = pin_bytes
        .as_bytes()
        .iter()
        .take(4)
        .map(|b| char::from(b'0' + (b % 10) as u8))
        .collect();

    PairingCode {
        code,
        qr_payload: payload.to_uri(),
        expires_in_s: PIN_TTL.as_secs() as u32,
        rotated_at_ms: now_ms(),
    }
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
        capture_backend: ui.capture_backend.clone(),
        slices: ui.slices,
        sfe: ui.sfe,
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
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
