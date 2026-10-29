//! 前后端共享的 DTO。
//!
//! §3.11:桌面端只通过 commands/events 与 Rust 交互,视频帧不经 IPC,
//! 因此这里全部是控制面/统计面的轻量结构。
//! 字段统一 `camelCase` 序列化,与前端 `src/ipc/types.ts` 一一对应。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 主机(Host)
// ---------------------------------------------------------------------------

/// 后端实现来源。`stub` 表示当前是本地桩实现(crates/ 尚未接线),
/// 前端会明确标注,避免把演示数据误当作实测值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    /// 本地内存桩实现,数据为演示用合成值。
    Stub,
    /// 已接 crates/beam-host / beam-client。
    BeamCrates,
}

/// §3.9 会话状态机的桌面端子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostState {
    /// 待机。
    Idle,
    /// 已开播,等待客户端接入。
    Hosting,
    /// 已有客户端串流中。
    Streaming,
    /// 异常,`HostStatus::message` 带说明。
    Error,
}

/// `get_host_status` 的返回(§3.11)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub state: HostState,
    /// 本机主机名,用于 mDNS 广播与列表展示。
    pub device_name: String,
    pub local_ip: Option<String>,
    /// 局域网监听端口(§3.11 设置项「端口 47990 / UDP」)。
    pub port: u16,
    pub gpu: Option<String>,
    /// 实际生效的硬编码器,如 `NVENC`。
    pub encoder: Option<String>,
    /// 实际生效的编码格式,如 `HEVC`。
    pub codec: Option<String>,
    pub resolution: String,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub discovery_enabled: bool,
    /// 当前串流中的客户端设备(有则说明已有人连上)。
    pub connected_device: Option<Device>,
    /// 状态说明,仅 `state == error` 时有意义。
    pub message: Option<String>,
    pub backend: BackendKind,
    /// 单调递增的版本号,前端可用它判断是否需要重绘。
    pub revision: u64,
    pub updated_at_ms: u64,
}

/// `start_hosting(opts)` 的参数(§3.11)。
///
/// 所有字段可选:缺省时沿用 `Settings` 中的当前值(§3.14 配置优先级)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHostingOptions {
    pub resolution: Option<String>,
    pub fps: Option<u32>,
    pub codec: Option<String>,
    pub bitrate_mbps: Option<u32>,
    /// 虚拟显示器(§3.5 抓屏来源,对应 v1 的 IddCx 驱动)。
    pub virtual_display: Option<bool>,
    /// 虚拟 Xbox 手柄(§3.8 beam-input / ViGEm)。
    pub gamepad: Option<bool>,
    /// 音频模式:`host+stream` / `stream_only` / `off`(§3.7)。
    pub audio_mode: Option<String>,
}

// ---------------------------------------------------------------------------
// 配对(Pair)
// ---------------------------------------------------------------------------

/// `get_pairing_code` 的返回(§3.11 / §3.3)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    /// 4 位数字 PIN(§3.3 `pin_len=4`)。
    pub code: String,
    /// 二维码载荷:`beam://pair?h=ip:port&fp=..&t=token`(§3.3)。
    pub qr_payload: String,
    /// 剩余有效秒数(§3.3 `pin_ttl=120s`)。
    pub expires_in_s: u32,
    pub rotated_at_ms: u64,
}

/// `pair://request` 事件负载:一条待确认的配对请求(§3.11)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub address: String,
    /// 证书指纹(§3.3 `Fingerprint([u8;32])`)。
    pub fingerprint: String,
    pub requested_at_ms: u64,
}

// ---------------------------------------------------------------------------
// 设备(Device)
// ---------------------------------------------------------------------------

/// 设备的输入权限(§3.8 权限控制)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePerms {
    pub view: bool,
    pub keyboard: bool,
    pub mouse: bool,
    pub gamepad: bool,
}

/// 最近一次会话摘要(设备列表里的「上次 连接时间 · 画质 · 平均延迟」)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastSession {
    /// `YYYY-MM-DD HH:mm` 本地时间。
    pub at: String,
    pub resolution: String,
    pub fps: u32,
    pub avg_latency_ms: Option<f32>,
}

/// 已配对设备(§3.3 信任库 `TrustedPeer`)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    /// `Android 15` / `Windows 11` / `Android TV 12` 等。
    pub platform: String,
    pub fingerprint: String,
    /// `YYYY-MM-DD` 配对日期。
    pub paired_at: String,
    pub online: bool,
    /// 该设备当前是否正与本机串流。
    pub streaming: bool,
    pub last_session: Option<LastSession>,
    pub perms: DevicePerms,
}

// ---------------------------------------------------------------------------
// 发现(Discovery)
// ---------------------------------------------------------------------------

/// `list_hosts` 的单条结果(§3.4 mDNS `_beam._udp.local`)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredHost {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    /// 是否已在本地信任库中(§3.3)。
    pub trusted: bool,
    pub version: String,
    pub gpu: Option<String>,
    pub online: bool,
}

// ---------------------------------------------------------------------------
// 会话(Session)
// ---------------------------------------------------------------------------

/// `connect(host_id)` 的返回(§3.9 `ClientSession::connect`)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub host_id: String,
    pub host_name: String,
    /// `connecting` / `streaming` / `reconnecting`。
    pub state: String,
    pub resolution: String,
    pub fps: u32,
    pub codec: String,
    pub started_at_ms: u64,
}

/// `session://stats` 事件负载(§3.11,500 ms 一拍)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub session_id: String,
    /// 端到端延迟(§1.2:主机抓屏时刻 → 客户端上屏时刻)。
    pub e2e_ms: f32,
    pub capture_ms: f32,
    pub color_convert_ms: f32,
    pub encode_ms: f32,
    /// 单程网络 + RTT/2。
    pub network_ms: f32,
    pub decode_ms: f32,
    pub render_ms: f32,
    pub fps: u32,
    pub bitrate_mbps: f32,
    pub loss_pct: f32,
    pub fec_recovered_pct: f32,
    pub jitter_ms: f32,
    pub dropped_frames: u32,
    pub resolution: String,
    pub codec: String,
    pub timestamp_ms: u64,
}

/// `session://reconnecting` 事件负载。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectingInfo {
    pub session_id: String,
    /// 已重连时长(§3.9 `reconnect_timeout=30s`)。
    pub elapsed_ms: u64,
    /// 保留最后一帧画面。
    pub keep_last_frame: bool,
}

/// `session://disconnected` 事件负载。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectedInfo {
    pub session_id: String,
    /// 断开原因;正常断开为 `by_user`。
    pub reason: String,
    pub code: Option<String>,
}

// ---------------------------------------------------------------------------
// 设置(§3.14)
// ---------------------------------------------------------------------------

/// 完整设置快照,`get_settings` 返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // 画质
    pub resolution: String,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub hdr: bool,
    /// 虚拟显示器:匹配客户端分辨率。
    pub match_client_resolution: bool,
    // 编码
    /// `auto` / `nvenc` / `amf` / `qsv`(§3.6)。
    pub encoder: String,
    /// `hevc` / `h264` / `av1`(§3.6)。
    pub codec: String,
    /// `low_latency_p1` / `balanced` / `quality`。
    pub preset: String,
    /// `auto` / `wgc` / `dxgi`(§3.5)。
    pub capture_backend: String,
    pub slices: u32,
    pub sfe: bool,
    // 网络
    /// FEC 冗余比例(§3.2 默认 0.2)。
    pub fec_ratio: f32,
    pub port: u16,
    pub allow_tailscale: bool,
    pub discovery_enabled: bool,
    pub bitrate_min_mbps: u32,
    pub bitrate_max_mbps: u32,
    // 音频(§3.7)
    pub audio_mode: String,
    pub audio_bitrate_kbps: u32,
    // 界面
    /// `zh-CN` / `en`(§2 要求中文优先,结构上预留 en)。
    pub language: String,
    /// `dark` / `light` / `system`。
    pub theme: String,
    pub show_stats_overlay: bool,
    pub minimize_to_tray: bool,
    pub launch_on_startup: bool,
}

/// `set_settings(patch)` 的参数:只带需要改的字段(§3.14 增量合并)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub resolution: Option<String>,
    pub fps: Option<u32>,
    pub bitrate_mbps: Option<u32>,
    pub hdr: Option<bool>,
    pub match_client_resolution: Option<bool>,
    pub encoder: Option<String>,
    pub codec: Option<String>,
    pub preset: Option<String>,
    pub capture_backend: Option<String>,
    pub slices: Option<u32>,
    pub sfe: Option<bool>,
    pub fec_ratio: Option<f32>,
    pub port: Option<u16>,
    pub allow_tailscale: Option<bool>,
    pub discovery_enabled: Option<bool>,
    pub bitrate_min_mbps: Option<u32>,
    pub bitrate_max_mbps: Option<u32>,
    pub audio_mode: Option<String>,
    pub audio_bitrate_kbps: Option<u32>,
    pub language: Option<String>,
    pub theme: Option<String>,
    pub show_stats_overlay: Option<bool>,
    pub minimize_to_tray: Option<bool>,
    pub launch_on_startup: Option<bool>,
}

// ---------------------------------------------------------------------------
// 性能统计(§3.13,仅 Debug)
// ---------------------------------------------------------------------------

/// 单个阶段的耗时统计(均值 / P50 / P99)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfStageStat {
    /// 阶段名:`capture` / `color_convert` / `encode` / `packetize` / `decode` / `render` …
    pub stage: String,
    pub mean_ms: f32,
    pub p50_ms: f32,
    pub p99_ms: f32,
}

/// 一个汇总指标(帧率、码率、CPU…)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfMetric {
    pub name: String,
    pub value: String,
    pub unit: String,
}

/// `perf_snapshot` 的返回(§3.11 / §3.13)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfSnapshot {
    /// 是否可用:release 构建下为 false(§3.13 门控)。
    pub available: bool,
    pub stages: Vec<PerfStageStat>,
    pub metrics: Vec<PerfMetric>,
    /// 端到端延迟序列,用于画折线。
    pub e2e_series: Vec<f32>,
    pub samples: u32,
    pub captured_at_ms: u64,
}

/// `perf://tick` 事件负载(§3.11,250 ms 一拍,仅 Debug)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfTick {
    pub e2e_ms: f32,
    pub fps: u32,
    pub bitrate_mbps: f32,
    pub cpu_pct: f32,
    pub gpu_pct: f32,
    pub memory_mb: u32,
    pub timestamp_ms: u64,
}

/// `perf_export(format)` 的返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfExportResult {
    /// `csv` / `json`。
    pub format: String,
    /// 导出文件的绝对路径。
    pub path: String,
    pub bytes: u64,
    pub rows: u32,
}
