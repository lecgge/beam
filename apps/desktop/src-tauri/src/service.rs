//! 服务层:桌面端与 Rust 核心之间的接缝。
//!
//! §3.11 规定桌面端只通过 commands/events 与 Rust 交互;§4 分层要求
//! `apps/desktop → beam-host / beam-client`。
//!
//! 本文件定义两层:
//!
//! 1. [`BeamService`] —— 与 crate 无关的 trait,即接线接缝。
//!    crates/beam-host、beam-client 落地后,只需新增一个实现该 trait 的适配器,
//!    命令层(commands.rs)与事件层(events.rs)完全不用改。
//! 2. [`LocalService`] —— 本地内存实现,当前默认启用。
//!    它让整个桌面端在没有 crates/ 的情况下也能真实跑通全部交互流程,
//!    但其中**主机名 / 本机 IP / 设置项是真实取值**,而**设备列表、发现列表、
//!    延迟统计是合成的演示数据**——后端会通过 `BackendKind::Stub` 告知前端,
//!    前端在界面上明确标注「演示数据」,避免被误读为实测值。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::dto::*;
use crate::error::*;

/// §3.2 `connect_timeout`:QUIC 握手等待上限。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 传输层握手结果(§3.2),由 `crates/` 适配器的后台任务回报。
///
/// 桌面端的 `connect` command 需要同步给出成败,因此握手在后台进行,
/// 结果通过这个结构体交回事件层。
#[derive(Debug, Clone)]
pub struct TransportOutcome {
    /// 握手是否成功。
    pub ok: bool,
    /// 失败时的错误码(成功时为 `None`)。
    pub code: Option<String>,
    /// 面向日志的说明。
    pub message: String,
    /// 对端主机名。
    pub host_name: String,
}

/// 真实客户端会话产生的事件(§3.9)。
///
/// `crates/` 适配器把 `beam-session` 的事件翻译成这一层,
/// 桌面端据此更新会话状态并广播 §3.11 的事件。
#[derive(Debug, Clone)]
pub enum SessionEventInfo {
    /// 能力协商完成,进入串流态(§3.1)。
    Negotiated {
        /// 协商出的分辨率宽。
        width: u16,
        /// 协商出的分辨率高。
        height: u16,
        /// 协商出的帧率。
        fps: u32,
        /// 协商出的编码格式。
        codec: String,
    },
    /// 配对完成(§3.3)。
    Paired {
        /// 对端证书指纹。
        fingerprint: String,
    },
    /// 会话统计(§3.11 `session://stats`)。
    Stats {
        /// 对端上报的帧率。
        fps: u32,
        /// 对端上报的码率,bit/s。
        bitrate_bps: u32,
    },
    /// 会话出错(§6:UI 只按 code 显示文案)。
    Error {
        /// 错误码。
        code: u16,
        /// 是否值得重试。
        retryable: bool,
    },
    /// 会话结束。
    Closed,
}

/// 与 crate 无关的服务接口。接线接缝。
pub trait BeamService: Send + Sync + 'static {
    /// 当前后端实现来源。
    fn backend(&self) -> BackendKind;
    /// 主机状态快照。
    fn host_status(&self) -> HostStatus;
    /// 开播(§3.11 `start_hosting`)。
    fn start_hosting(&self, opts: StartHostingOptions) -> Result<HostStatus, BeamError>;
    /// 停止串流(§3.11 `stop_hosting`)。
    fn stop_hosting(&self) -> Result<HostStatus, BeamError>;
    /// 当前配对码(§3.11 `get_pairing_code`)。
    fn pairing_code(&self) -> PairingCode;
    /// 同意配对(§3.11 `approve_pair`)。
    fn approve_pair(&self, id: &str) -> Result<Device, BeamError>;
    /// 拒绝配对(§3.11 `reject_pair`)。
    fn reject_pair(&self, id: &str) -> Result<(), BeamError>;
    /// 移除已配对设备(§3.11 `remove_device`)。
    fn remove_device(&self, id: &str) -> Result<(), BeamError>;
    /// 已配对设备列表。
    fn devices(&self) -> Vec<Device>;
    /// 待确认的配对请求。
    fn pair_requests(&self) -> Vec<PairRequest>;
    /// 局域网发现到的主机(§3.11 `list_hosts`)。
    fn list_hosts(&self) -> Vec<DiscoveredHost>;
    /// 连接某台主机(§3.11 `connect`)。
    ///
    /// `pin` 是客户端输入的 4 位 PIN(§3.3 `pin_len=4`);已配对的主机可以传空串
    /// (信任库里已有指纹,不需要再输 PIN)。
    ///
    /// Rust 侧会做**格式校验**与**尝试预算**(§3.3 `max_attempts=3`),
    /// 见 [`crate::pin`]。SPAKE2 的密码学校验在主机侧(§3.3),尚未接线。
    fn connect(&self, host_id: &str, pin: &str) -> Result<SessionInfo, BeamError>;
    /// 断开当前会话(§3.11 `disconnect`)。
    fn disconnect(&self) -> Result<SessionInfo, BeamError>;
    /// 当前客户端会话。
    fn session(&self) -> Option<SessionInfo>;
    /// 当前会话统计(供 `session://stats` 500 ms 事件使用)。
    fn session_stats(&self) -> Option<SessionStats>;
    /// 会话刚进入重连状态时返回一次通知(§3.9 `Streaming → Reconnecting`),取走即清空。
    fn reconnect_notice(&self) -> Option<ReconnectingInfo>;
    /// 设置快照(§3.11 `get_settings`)。
    fn settings(&self) -> Settings;
    /// 增量更新设置(§3.11 `set_settings`,§3.14 优先级合并)。
    fn set_settings(&self, patch: SettingsPatch) -> Result<Settings, BeamError>;
    /// 性能统计快照(§3.11 `perf_snapshot`,§3.13)。
    fn perf_snapshot(&self) -> PerfSnapshot;
    /// 单拍性能数据(§3.11 `perf://tick`)。
    fn perf_tick(&self) -> Option<PerfTick>;
    /// 导出性能数据(§3.11 `perf_export`,§3.13 CSV/JSON)。
    fn perf_export(&self, format: &str) -> Result<PerfExportResult, BeamError>;
    /// 配对码是否已过期需要轮换(§3.3 `pin_ttl=120s`)。
    fn rotate_pairing_code_if_due(&self) -> Option<PairingCode>;
    /// 把主机状态置为 Error 并附带说明(托盘操作失败等)。
    fn set_host_error(&self, message: String);
    /// 取走后端传输层的一次握手结果(§3.2),取走即清空。
    ///
    /// 桩实现永远返回 `None`;`crates/` 适配器在 QUIC 握手结束后回报。
    fn take_transport_outcome(&self) -> Option<TransportOutcome>;

    /// 取走真实客户端会话产生的事件(§3.9),取走即清空。
    ///
    /// 桩实现没有真实会话,恒返回空。
    fn take_session_events(&self) -> Vec<SessionEventInfo> {
        Vec::new()
    }

    /// 取走**新出现**的配对请求(§3.3 主机端人工确认)。
    ///
    /// 与 [`BeamService::pair_requests`] 的区别:后者是全量快照,
    /// 本方法只返回「自上次调用以来新出现」的请求,供事件层发 `pair://request`。
    /// 桩实现恒返回空。
    fn take_new_pair_requests(&self) -> Vec<PairRequest> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// 本地实现
// ---------------------------------------------------------------------------

struct Inner {
    host: HostStatus,
    settings: Settings,
    devices: Vec<Device>,
    pair_requests: Vec<PairRequest>,
    discovered: Vec<DiscoveredHost>,
    pairing: PairingCode,
    pairing_token: String,
    session: Option<SessionInfo>,
    session_started: Option<Instant>,
    /// 已模拟发生的重连次数,用于让 `session://reconnecting` 只触发一次。
    reconnect_emitted: bool,
    /// 待上报的一次重连通知。
    reconnect_notice: Option<ReconnectingInfo>,
    revision: u64,
    /// PIN 尝试预算(§3.3 `max_attempts=3`):按主机 id 记账。
    pin_budget: crate::pin::AttemptBudget,
}

impl Inner {
    fn bump(&mut self) {
        self.revision += 1;
        self.host.revision = self.revision;
        self.host.updated_at_ms = now_ms();
        // 设置改动同步到主机状态,保证首页与设置页显示一致。
        self.host.resolution = self.settings.resolution.clone();
        self.host.fps = self.settings.fps;
        self.host.bitrate_mbps = self.settings.bitrate_mbps;
        self.host.codec = Some(self.settings.codec.to_uppercase());
        self.host.encoder = Some(self.settings.encoder.to_uppercase());
        self.host.discovery_enabled = self.settings.discovery_enabled;
        self.host.port = self.settings.port;
    }
}

/// 本地内存实现:当前默认。
pub struct LocalService {
    inner: Mutex<Inner>,
}

impl LocalService {
    pub fn new() -> Self {
        let now = now_ms();
        let settings = default_settings();
        let device_name = device_name();
        let host = HostStatus {
            state: HostState::Idle,
            device_name: device_name.clone(),
            local_ip: detect_local_ip(),
            port: settings.port,
            gpu: None,
            encoder: Some(settings.encoder.to_uppercase()),
            codec: Some(settings.codec.to_uppercase()),
            resolution: settings.resolution.clone(),
            fps: settings.fps,
            bitrate_mbps: settings.bitrate_mbps,
            discovery_enabled: settings.discovery_enabled,
            connected_device: None,
            message: None,
            backend: BackendKind::Stub,
            revision: 0,
            updated_at_ms: now,
        };

        let pairing = make_pairing_code(&device_name, settings.port, now);

        let inner = Inner {
            host,
            settings,
            devices: seed_devices(now),
            pair_requests: seed_pair_requests(now),
            discovered: seed_discovered(),
            pairing,
            pairing_token: "tok-demo".to_string(),
            session: None,
            session_started: None,
            reconnect_emitted: false,
            reconnect_notice: None,
            revision: 0,
            pin_budget: crate::pin::AttemptBudget::new(),
        };

        Self {
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 桌面端是单用户应用,锁中毒时继续使用既有状态而不是整个进程崩掉。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for LocalService {
    fn default() -> Self {
        Self::new()
    }
}

impl BeamService for LocalService {
    fn backend(&self) -> BackendKind {
        BackendKind::Stub
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
        // §3.14:命令行/环境 > 会话内临时覆盖 > 用户配置 > 能力推导默认值。
        // 这里的 opts 属于「会话内临时覆盖」。
        if let Some(v) = opts.resolution {
            inner.settings.resolution = v;
        }
        if let Some(v) = opts.fps {
            inner.settings.fps = v;
        }
        if let Some(v) = opts.codec {
            inner.settings.codec = v;
        }
        if let Some(v) = opts.bitrate_mbps {
            inner.settings.bitrate_mbps = v;
        }
        if let Some(v) = opts.audio_mode {
            inner.settings.audio_mode = v;
        }
        inner.host.state = HostState::Hosting;
        inner.host.message = None;
        inner.bump();
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
        inner.host.state = HostState::Idle;
        inner.host.connected_device = None;
        inner.host.message = None;
        inner.bump();
        Ok(inner.host.clone())
    }

    fn pairing_code(&self) -> PairingCode {
        let mut inner = self.lock();
        let now = now_ms();
        let elapsed = now.saturating_sub(inner.pairing.rotated_at_ms);
        inner.pairing.expires_in_s = ((PAIR_PIN_TTL_MS.saturating_sub(elapsed)) / 1000) as u32;
        inner.pairing.clone()
    }

    fn approve_pair(&self, id: &str) -> Result<Device, BeamError> {
        let mut inner = self.lock();
        let idx = inner
            .pair_requests
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| {
                BeamError::invalid(E_DEVICE_NOT_FOUND, format!("配对请求 {id} 不存在"))
            })?;
        let req = inner.pair_requests.remove(idx);
        let now = now_ms();
        let device = Device {
            id: req.id.clone(),
            name: req.name.clone(),
            platform: req.platform.clone(),
            fingerprint: req.fingerprint.clone(),
            paired_at: format_date(now),
            online: true,
            streaming: false,
            last_session: None,
            perms: DevicePerms {
                view: true,
                keyboard: true,
                mouse: true,
                gamepad: true,
            },
        };
        inner.devices.push(device.clone());
        inner.bump();
        Ok(device)
    }

    fn reject_pair(&self, id: &str) -> Result<(), BeamError> {
        let mut inner = self.lock();
        let before = inner.pair_requests.len();
        inner.pair_requests.retain(|r| r.id != id);
        if inner.pair_requests.len() == before {
            return Err(BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("配对请求 {id} 不存在"),
            ));
        }
        inner.bump();
        Ok(())
    }

    fn remove_device(&self, id: &str) -> Result<(), BeamError> {
        let mut inner = self.lock();
        let before = inner.devices.len();
        inner.devices.retain(|d| d.id != id);
        if inner.devices.len() == before {
            return Err(BeamError::invalid(
                E_DEVICE_NOT_FOUND,
                format!("设备 {id} 不存在"),
            ));
        }
        inner.bump();
        Ok(())
    }

    fn devices(&self) -> Vec<Device> {
        self.lock().devices.clone()
    }

    fn pair_requests(&self) -> Vec<PairRequest> {
        self.lock().pair_requests.clone()
    }

    fn list_hosts(&self) -> Vec<DiscoveredHost> {
        self.lock().discovered.clone()
    }

    fn connect(&self, host_id: &str, pin: &str) -> Result<SessionInfo, BeamError> {
        let mut inner = self.lock();
        if inner.session.is_some() {
            return Err(BeamError::invalid(
                E_SESSION_ALREADY_ACTIVE,
                "已经有一条串流会话,请先断开",
            ));
        }
        let host = inner
            .discovered
            .iter()
            .find(|h| h.id == host_id)
            .cloned()
            .ok_or_else(|| {
                BeamError::retryable(
                    E_HOST_NOT_FOUND,
                    format!("未发现主机 {host_id},请刷新后重试"),
                )
            })?;
        if !host.online {
            return Err(BeamError::retryable(
                E_CONNECTION_TIMEOUT,
                format!("主机 {} 当前离线", host.name),
            ));
        }
        // §3.3:PIN 准入(格式校验 + 尝试预算)。未信任的主机必须给 PIN。
        let now = Instant::now();
        if let Err(err) = crate::pin::check_connect_pin(
            &inner.pin_budget,
            host_id,
            host.trusted,
            pin,
            now,
        ) {
            // 格式错误算一次尝试(§3.3 `max_attempts=3`),锁定则如实报 E3002。
            if err.code == E_PIN_INVALID {
                if let Err(locked) = inner.pin_budget.record_failure(host_id, now) {
                    return Err(BeamError::invalid(E_PIN_LOCKED, locked));
                }
            }
            return Err(err);
        }
        let now = now_ms();
        let session = SessionInfo {
            session_id: format!("s-{:x}", now),
            host_id: host.id.clone(),
            host_name: host.name.clone(),
            state: "streaming".to_string(),
            resolution: inner.settings.resolution.clone(),
            fps: inner.settings.fps,
            codec: inner.settings.codec.to_uppercase(),
            started_at_ms: now,
        };
        inner.session = Some(session.clone());
        inner.session_started = Some(Instant::now());
        inner.reconnect_emitted = false;
        inner.reconnect_notice = None;
        // 连接成功 ⇒ 该主机的 PIN 预算复位。
        inner.pin_budget.reset(host_id);
        inner.bump();
        Ok(session)
    }

    fn disconnect(&self) -> Result<SessionInfo, BeamError> {
        let mut inner = self.lock();
        let session = inner
            .session
            .take()
            .ok_or_else(|| BeamError::invalid(E_SESSION_INVALID_STATE, "当前没有串流会话"))?;
        inner.session_started = None;
        inner.bump();
        Ok(session)
    }

    fn session(&self) -> Option<SessionInfo> {
        self.lock().session.clone()
    }

    fn session_stats(&self) -> Option<SessionStats> {
        let mut inner = self.lock();
        let session = inner.session.clone()?;
        let started = inner.session_started.unwrap_or_else(Instant::now);
        let t = started.elapsed().as_secs_f32();
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let session_id = session.session_id.clone();

        // 桩实现:模拟一次 §3.9 的 `Streaming → Reconnecting → Streaming` 抖动,
        // 让前端能真实跑通重连提示路径。真实实现由 crates/beam-session 驱动。
        let in_reconnect_window = (RECONNECT_SIM_AT_MS
            ..RECONNECT_SIM_AT_MS + RECONNECT_SIM_WINDOW_MS)
            .contains(&elapsed_ms);
        if let Some(s) = inner.session.as_mut() {
            s.state = if in_reconnect_window {
                "reconnecting".to_string()
            } else {
                "streaming".to_string()
            };
        }
        if in_reconnect_window {
            if !inner.reconnect_emitted {
                inner.reconnect_emitted = true;
                inner.reconnect_notice = Some(ReconnectingInfo {
                    session_id: session_id.clone(),
                    elapsed_ms,
                    keep_last_frame: true,
                });
            }
        } else {
            inner.reconnect_emitted = false;
        }

        // 合成统计:数值围绕原型给出的典型量级小幅波动,便于前端图表有可看的曲线。
        // 注意:这是桩实现的演示数据,不是实测值(backend == Stub 时前端会标注)。
        let wobble = (t * 1.9).sin() * 0.8 + (t * 0.53).sin() * 0.35;
        let capture_ms = 2.4 + wobble * 0.25;
        let color_convert_ms = 0.6 + wobble * 0.08;
        let encode_ms = 3.9 + wobble * 0.45;
        let network_ms = 3.2 + wobble * 0.30;
        let decode_ms = 4.3 + wobble * 0.40;
        let render_ms = 4.0 + wobble * 0.35;
        let e2e_ms = capture_ms + color_convert_ms + encode_ms + network_ms + decode_ms + render_ms;

        let fps = inner.settings.fps;
        let bitrate_mbps =
            (inner.settings.bitrate_mbps as f32 * (1.0 + wobble * 0.02).clamp(0.85, 1.15)).round();
        let loss_pct = (0.4 + wobble * 0.05).max(0.0);
        let jitter_ms = (1.1 + wobble * 0.12).max(0.0);

        Some(SessionStats {
            session_id,
            e2e_ms: (e2e_ms * 10.0).round() / 10.0,
            capture_ms: (capture_ms * 10.0).round() / 10.0,
            color_convert_ms: (color_convert_ms * 10.0).round() / 10.0,
            encode_ms: (encode_ms * 10.0).round() / 10.0,
            network_ms: (network_ms * 10.0).round() / 10.0,
            decode_ms: (decode_ms * 10.0).round() / 10.0,
            render_ms: (render_ms * 10.0).round() / 10.0,
            fps,
            bitrate_mbps,
            loss_pct: (loss_pct * 100.0).round() / 100.0,
            fec_recovered_pct: 100.0,
            jitter_ms: (jitter_ms * 100.0).round() / 100.0,
            dropped_frames: 0,
            resolution: inner.settings.resolution.clone(),
            codec: inner.settings.codec.to_uppercase(),
            timestamp_ms: now_ms(),
        })
    }

    fn reconnect_notice(&self) -> Option<ReconnectingInfo> {
        self.lock().reconnect_notice.take()
    }

    fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    fn set_settings(&self, patch: SettingsPatch) -> Result<Settings, BeamError> {
        let mut inner = self.lock();
        // §3.14 校验:先做取值范围检查,非法值返回 E11001,不静默改写。
        if let Some(res) = &patch.resolution {
            if !RESOLUTIONS.contains(&res.as_str()) {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("不支持的分辨率 {res}"),
                ));
            }
        }
        if let Some(fps) = patch.fps {
            if !FRAMERATES.contains(&fps) {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("不支持的帧率 {fps}"),
                ));
            }
        }
        if let Some(codec) = &patch.codec {
            if !CODECS.contains(&codec.as_str()) {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("不支持的编码格式 {codec}"),
                ));
            }
        }
        if let Some(enc) = &patch.encoder {
            if !ENCODERS.contains(&enc.as_str()) {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("不支持的编码器 {enc}"),
                ));
            }
        }
        if let Some(ratio) = patch.fec_ratio {
            if !(0.0..=0.5).contains(&ratio) {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("FEC 冗余 {ratio} 超出 0.0–0.5"),
                ));
            }
        }
        if let Some(port) = patch.port {
            // u16 上界 65535 由类型保证,这里只需挡掉保留端口以下的值。
            if port < 1024 {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("端口 {port} 非法"),
                ));
            }
        }

        let s = &mut inner.settings;
        macro_rules! apply {
            ($field:ident) => {
                if let Some(v) = patch.$field {
                    s.$field = v;
                }
            };
        }
        apply!(resolution);
        apply!(fps);
        apply!(bitrate_mbps);
        apply!(hdr);
        apply!(match_client_resolution);
        apply!(encoder);
        apply!(codec);
        apply!(preset);
        apply!(capture_backend);
        apply!(slices);
        apply!(sfe);
        apply!(fec_ratio);
        apply!(port);
        apply!(allow_tailscale);
        apply!(discovery_enabled);
        apply!(bitrate_min_mbps);
        apply!(bitrate_max_mbps);
        apply!(audio_mode);
        apply!(audio_bitrate_kbps);
        apply!(language);
        apply!(theme);
        apply!(show_stats_overlay);
        apply!(minimize_to_tray);
        apply!(launch_on_startup);

        inner.bump();
        Ok(inner.settings.clone())
    }

    fn perf_snapshot(&self) -> PerfSnapshot {
        let inner = self.lock();
        let e2e = 17.8;
        let stages = vec![
            stage("capture", 2.4, 4.0),
            stage("color_convert", 0.6, 1.0),
            stage("encode", 3.9, 6.0),
            stage("packetize", 0.2, 0.4),
            stage("network", 3.2, 5.0),
            stage("jitter_buffer", 0.8, 2.0),
            stage("decode", 4.3, 7.0),
            stage("render", 4.0, 6.0),
        ];
        let e2e_series: Vec<f32> = (0..16)
            .map(|i| {
                let x = i as f32;
                e2e + (x * 0.9).sin() * 1.6 + (x * 0.31).sin() * 0.9
            })
            .map(|v| (v * 10.0).round() / 10.0)
            .collect();
        let metrics = vec![
            PerfMetric {
                name: "fps".into(),
                value: inner.settings.fps.to_string(),
                unit: "fps".into(),
            },
            PerfMetric {
                name: "bitrate".into(),
                value: inner.settings.bitrate_mbps.to_string(),
                unit: "Mbps".into(),
            },
            PerfMetric {
                name: "loss".into(),
                value: "0.3".into(),
                unit: "%".into(),
            },
            PerfMetric {
                name: "fec_recovered".into(),
                value: "100".into(),
                unit: "%".into(),
            },
            PerfMetric {
                name: "cpu".into(),
                value: "7".into(),
                unit: "%".into(),
            },
            PerfMetric {
                name: "gpu_encode".into(),
                value: "34".into(),
                unit: "%".into(),
            },
            PerfMetric {
                name: "memory".into(),
                value: "182".into(),
                unit: "MB".into(),
            },
            PerfMetric {
                name: "rtt".into(),
                value: "0.8".into(),
                unit: "ms".into(),
            },
        ];
        PerfSnapshot {
            available: true,
            stages,
            metrics,
            e2e_series,
            samples: 16,
            captured_at_ms: now_ms(),
        }
    }

    fn perf_tick(&self) -> Option<PerfTick> {
        // 注意:session_stats() 自身会取锁,这里不能再持有锁(std::sync::Mutex 不可重入)。
        let stats = self.session_stats()?;
        Some(PerfTick {
            e2e_ms: stats.e2e_ms,
            fps: stats.fps,
            bitrate_mbps: stats.bitrate_mbps,
            cpu_pct: 7.0,
            gpu_pct: 34.0,
            memory_mb: 182,
            timestamp_ms: stats.timestamp_ms,
        })
    }

    fn perf_export(&self, format: &str) -> Result<PerfExportResult, BeamError> {
        let snap = self.perf_snapshot();
        let dir = export_dir()?;
        let stamp = now_ms();
        let (ext, body) = match format {
            "csv" => ("csv", perf_csv(&snap)),
            "json" => ("json", perf_json(&snap)),
            other => {
                return Err(BeamError::invalid(
                    E_SHELL_CONFIG_INVALID,
                    format!("不支持的导出格式 {other},仅支持 csv / json"),
                ))
            }
        };
        let path = dir.join(format!("beam-perf-{stamp}.{ext}"));
        std::fs::write(&path, body.as_bytes())
            .map_err(|e| BeamError::new(E_SHELL_IO, format!("写入导出文件失败: {e}"), false))?;
        Ok(PerfExportResult {
            format: format.to_string(),
            path: path.to_string_lossy().to_string(),
            bytes: body.len() as u64,
            rows: snap.samples,
        })
    }

    fn rotate_pairing_code_if_due(&self) -> Option<PairingCode> {
        let mut inner = self.lock();
        let now = now_ms();
        if now.saturating_sub(inner.pairing.rotated_at_ms) < PAIR_PIN_TTL_MS {
            return None;
        }
        let code = make_pairing_code(&inner.host.device_name, inner.settings.port, now);
        inner.pairing = code.clone();
        inner.bump();
        Some(code)
    }

    fn set_host_error(&self, message: String) {
        let mut inner = self.lock();
        inner.host.state = HostState::Error;
        inner.host.message = Some(message);
        inner.bump();
    }

    fn take_transport_outcome(&self) -> Option<TransportOutcome> {
        // 桩实现没有真实传输层,不会有握手结果。
        None
    }
}

// ---------------------------------------------------------------------------
// 实现选择
// ---------------------------------------------------------------------------

/// 按 feature 选择服务实现:
///
/// - `beam-crates`(默认):走 `crates/` 适配器,设置、配对、信任库、
///   mDNS 发现、传输层都是真实实现;
/// - 关闭 feature:退回本地内存桩,保证 `crates/` 缺席时桌面端仍可独立构建。
pub fn create_service() -> Arc<dyn BeamService> {
    #[cfg(feature = "beam-crates")]
    {
        Arc::new(crate::service_crates::CrateService::new())
    }
    #[cfg(not(feature = "beam-crates"))]
    {
        Arc::new(LocalService::new())
    }
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// §3.3 `pin_ttl=120s`。
pub const PAIR_PIN_TTL_MS: u64 = 120_000;
/// §3.11 默认端口 47990 / UDP。
pub const DEFAULT_PORT: u16 = 47990;
/// 桩实现:会话开始多久后模拟一次抖动(§3.9 `stall_threshold=3s` 的演示放大版)。
pub const RECONNECT_SIM_AT_MS: u64 = 20_000;
/// 桩实现:抖动窗口长度。
pub const RECONNECT_SIM_WINDOW_MS: u64 = 2_000;

const RESOLUTIONS: &[&str] = &["720p", "1080p", "1440p", "4k"];
const FRAMERATES: &[u32] = &[30, 60, 90, 120, 165, 200];
const CODECS: &[&str] = &["hevc", "h264", "av1"];
const ENCODERS: &[&str] = &["auto", "nvenc", "amf", "qsv"];

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

/// 取本机在局域网侧的 IPv4。
///
/// 用 UDP `connect` 只做路由选择,不发送任何数据报,因此离线环境也安全;
/// 取不到就返回 None,由前端显示「未获取到」而不是编造地址。
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

/// 4 位 PIN + 二维码载荷(§3.3)。
fn make_pairing_code(host: &str, port: u16, now: u64) -> PairingCode {
    let seed = now
        .wrapping_mul(2_654_435_761)
        .wrapping_add(host.len() as u64)
        .wrapping_add(port as u64);
    let code = format!("{:04}", seed % 10_000);
    let fp = format!("{:08X}", (seed >> 8) & 0xffff_ffff);
    let token = format!("{:016x}", seed.rotate_left(17));
    PairingCode {
        qr_payload: format!("beam://pair?h={host}:{port}&fp={fp}&t={token}"),
        code,
        expires_in_s: (PAIR_PIN_TTL_MS / 1000) as u32,
        rotated_at_ms: now,
    }
}

/// §3.14 优先级链最底层的内置默认值。
fn default_settings() -> Settings {
    Settings {
        resolution: "1080p".into(),
        fps: 60,
        bitrate_mbps: 20,
        hdr: false,
        match_client_resolution: true,
        encoder: "auto".into(),
        codec: "hevc".into(),
        preset: "low_latency_p1".into(),
        capture_backend: "auto".into(),
        slices: 1,
        sfe: false,
        fec_ratio: 0.2,
        port: DEFAULT_PORT,
        allow_tailscale: false,
        discovery_enabled: true,
        bitrate_min_mbps: 5,
        bitrate_max_mbps: 50,
        audio_mode: "host+stream".into(),
        audio_bitrate_kbps: 128,
        language: "zh-CN".into(),
        theme: "dark".into(),
        show_stats_overlay: true,
        minimize_to_tray: true,
        launch_on_startup: false,
        // §3.8：客户端键鼠回传默认关（敏感操作，用户主动开启）。
        share_input: false,
    }
}

/// 演示用设备列表(合成数据,非实测)。
fn seed_devices(now: u64) -> Vec<Device> {
    vec![
        Device {
            id: "dev-redmi-k70".into(),
            name: "Redmi K70".into(),
            platform: "Android 15".into(),
            fingerprint: "9F:2A:1C:4D:…:C1".into(),
            paired_at: format_date(now - 9 * 86_400_000),
            online: true,
            streaming: false,
            last_session: Some(LastSession {
                at: format_datetime(now - 3_600_000),
                resolution: "1080p".into(),
                fps: 60,
                avg_latency_ms: Some(18.0),
            }),
            perms: DevicePerms {
                view: true,
                keyboard: true,
                mouse: true,
                gamepad: false,
            },
        },
        Device {
            id: "dev-tcl-tv".into(),
            name: "客厅 TCL 电视".into(),
            platform: "Android TV 12".into(),
            fingerprint: "3B:77:9A:02:…:0E".into(),
            paired_at: format_date(now - 17 * 86_400_000),
            online: false,
            streaming: false,
            last_session: Some(LastSession {
                at: format_datetime(now - 86_400_000),
                resolution: "4k".into(),
                fps: 60,
                avg_latency_ms: Some(24.0),
            }),
            perms: DevicePerms {
                view: true,
                keyboard: false,
                mouse: false,
                gamepad: false,
            },
        },
        Device {
            id: "dev-thinkpad-x1".into(),
            name: "ThinkPad X1(Windows 客户端)".into(),
            platform: "Windows 11".into(),
            fingerprint: "A0:11:4E:83:…:5D".into(),
            paired_at: format_date(now - 29 * 86_400_000),
            online: false,
            streaming: false,
            last_session: None,
            perms: DevicePerms {
                view: true,
                keyboard: true,
                mouse: true,
                gamepad: true,
            },
        },
    ]
}

/// 演示用配对请求(合成数据)。
fn seed_pair_requests(now: u64) -> Vec<PairRequest> {
    vec![PairRequest {
        id: "pair-redmi-k70".into(),
        name: "Redmi K70".into(),
        platform: "Android 15".into(),
        address: "192.168.1.57".into(),
        fingerprint: "9F:2A:1C:4D:…:C1".into(),
        requested_at_ms: now,
    }]
}

/// 演示用发现列表(合成数据)。
fn seed_discovered() -> Vec<DiscoveredHost> {
    vec![
        DiscoveredHost {
            id: "host-game-pc".into(),
            name: "GAME-PC".into(),
            address: "192.168.1.80".into(),
            port: DEFAULT_PORT,
            trusted: false,
            version: "0.1.0".into(),
            gpu: Some("RTX 4070".into()),
            online: true,
            platform: "Windows 11".into(),
        },
        DiscoveredHost {
            id: "host-study-laptop".into(),
            name: "书房笔记本".into(),
            address: "192.168.1.91".into(),
            port: DEFAULT_PORT,
            trusted: true,
            version: "0.1.0".into(),
            gpu: None,
            online: false,
            platform: "Windows 11".into(),
        },
    ]
}

fn stage(name: &str, mean: f32, p99: f32) -> PerfStageStat {
    PerfStageStat {
        stage: name.into(),
        mean_ms: mean,
        p50_ms: (mean * 1.08 * 10.0).round() / 10.0,
        p99_ms: p99,
    }
}

/// §3.14:配置落盘位置 `%APPDATA%/Beam/`。
fn export_dir() -> Result<std::path::PathBuf, BeamError> {
    let base = std::env::var("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("Beam").join("exports");
    std::fs::create_dir_all(&dir)
        .map_err(|e| BeamError::new(E_SHELL_IO, format!("创建导出目录失败: {e}"), false))?;
    Ok(dir)
}

fn perf_csv(snap: &PerfSnapshot) -> String {
    let mut out = String::from("stage,mean_ms,p50_ms,p99_ms\n");
    for s in &snap.stages {
        out.push_str(&format!(
            "{},{},{},{}\n",
            s.stage, s.mean_ms, s.p50_ms, s.p99_ms
        ));
    }
    out
}

fn perf_json(snap: &PerfSnapshot) -> String {
    serde_json::json!({
        "available": snap.available,
        "samples": snap.samples,
        "capturedAtMs": snap.captured_at_ms,
        "stages": snap.stages,
        "metrics": snap.metrics,
        "e2eSeries": snap.e2e_series,
    })
    .to_string()
}

/// 民用日历转换(Howard Hinnant 算法),避免为格式化引入额外依赖。
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

fn format_datetime(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn format_date(ms: u64) -> String {
    let days = ((ms / 1000) as i64).div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}
