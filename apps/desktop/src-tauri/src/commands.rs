//! §3.11 host-app/client 的 command 层。
//!
//! §3.11 清单的 12 个 command 全部在此实现,Debug 构建下另有
//! `perf_snapshot` / `perf_export`(§3.13 perf-stats 门控)。
//!
//! 错误统一走 [`BeamError`],序列化为 `{code, message, retryable}`;
//! 成功直接返回值。
//!
//! 另补充 2 个清单外的只读 command(`list_devices` / `list_pair_requests`),
//! 因为 §3.11 的 12 个命令里没有读取已配对设备列表的入口,
//! 而「设备」页必须展示它。两者都是纯读取,不改变状态。

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::dto::*;
use crate::error::*;
use crate::events;
use crate::service::BeamService;

/// Tauri 托管的服务状态。
type Svc<'a> = State<'a, Arc<dyn BeamService>>;

// ---------------------------------------------------------------------------
// 主机
// ---------------------------------------------------------------------------

/// §3.11 `get_host_status`:读取本机主机状态。
#[tauri::command]
pub fn get_host_status(service: Svc<'_>) -> HostStatus {
    service.host_status()
}

/// §3.11 `start_hosting(opts)`:一键开播。
///
/// 成功后广播 `host://state-changed`(§3.11 事件清单)。
#[tauri::command]
pub fn start_hosting(
    app: AppHandle,
    service: Svc<'_>,
    opts: StartHostingOptions,
) -> Result<HostStatus, BeamError> {
    let status = service.start_hosting(opts)?;
    events::emit(&app, events::HOST_STATE_CHANGED, status.clone());
    crate::tray::sync_menu_state(&app, &status);
    Ok(status)
}

/// §3.11 `stop_hosting`:停止串流。
///
/// 成功后广播 `host://state-changed`。
#[tauri::command]
pub fn stop_hosting(app: AppHandle, service: Svc<'_>) -> Result<HostStatus, BeamError> {
    let status = service.stop_hosting()?;
    events::emit(&app, events::HOST_STATE_CHANGED, status.clone());
    crate::tray::sync_menu_state(&app, &status);
    Ok(status)
}

// ---------------------------------------------------------------------------
// 配对
// ---------------------------------------------------------------------------

/// §3.11 `get_pairing_code`:取当前配对码(4 位 PIN + 二维码载荷)。
///
/// PIN 过期轮换时前端会收到 `pair://code-rotated`(§3.3 `pin_ttl=120s`)。
#[tauri::command]
pub fn get_pairing_code(service: Svc<'_>) -> PairingCode {
    service.pairing_code()
}

/// §3.11 `approve_pair(id)`:同意一条配对请求。
#[tauri::command]
pub fn approve_pair(app: AppHandle, service: Svc<'_>, id: String) -> Result<Device, BeamError> {
    let device = service.approve_pair(&id)?;
    events::emit(&app, events::HOST_STATE_CHANGED, service.host_status());
    Ok(device)
}

/// §3.11 `reject_pair(id)`:拒绝一条配对请求。
#[tauri::command]
pub fn reject_pair(app: AppHandle, service: Svc<'_>, id: String) -> Result<(), BeamError> {
    service.reject_pair(&id)?;
    events::emit(&app, events::HOST_STATE_CHANGED, service.host_status());
    Ok(())
}

/// §3.11 `remove_device(id)`:移除已配对设备(删除后需重新配对)。
#[tauri::command]
pub fn remove_device(app: AppHandle, service: Svc<'_>, id: String) -> Result<(), BeamError> {
    service.remove_device(&id)?;
    events::emit(&app, events::HOST_STATE_CHANGED, service.host_status());
    Ok(())
}

// ---------------------------------------------------------------------------
// 发现与连接
// ---------------------------------------------------------------------------

/// §3.11 `list_hosts`:局域网发现到的主机列表(§3.4 mDNS)。
#[tauri::command]
pub fn list_hosts(service: Svc<'_>) -> Vec<DiscoveredHost> {
    service.list_hosts()
}

/// §3.11 `connect(host_id, pin)`:连接指定主机,建立客户端会话。
///
/// `pin` 是客户端输入的 4 位 PIN(§3.3 `pin_len=4`)。
/// **已配对的主机可以传空串** —— 信任库里已有它的证书指纹(§3.3),
/// 不需要再输 PIN。
///
/// Rust 侧会做格式校验与尝试预算(§3.3 `max_attempts=3`,超限报 `E3002`),
/// 见 `src/pin.rs`。SPAKE2 的密码学校验在主机侧(§3.3),尚未接线。
#[tauri::command]
pub fn connect(
    service: Svc<'_>,
    host_id: String,
    pin: Option<String>,
) -> Result<SessionInfo, BeamError> {
    service.connect(&host_id, pin.as_deref().unwrap_or(""))
}

/// §3.11 `disconnect`:断开当前客户端会话。
///
/// 成功后广播 `session://disconnected`。
#[tauri::command]
pub fn disconnect(app: AppHandle, service: Svc<'_>) -> Result<(), BeamError> {
    let session = service.disconnect()?;
    events::emit(
        &app,
        events::SESSION_DISCONNECTED,
        DisconnectedInfo {
            session_id: session.session_id,
            reason: "by_user".to_string(),
            code: None,
        },
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 设置(§3.14)
// ---------------------------------------------------------------------------

/// §3.11 `get_settings`:读取设置快照。
#[tauri::command]
pub fn get_settings(service: Svc<'_>) -> Settings {
    service.settings()
}

/// §3.11 `set_settings(patch)`:增量更新设置。
///
/// §3.14:命令行/环境变量 > 会话内临时覆盖 > 用户配置文件 > 能力推导默认值 > 内置默认。
#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    service: Svc<'_>,
    patch: SettingsPatch,
) -> Result<Settings, BeamError> {
    let settings = service.set_settings(patch)?;
    events::emit(&app, events::HOST_STATE_CHANGED, service.host_status());
    Ok(settings)
}

// ---------------------------------------------------------------------------
// 性能统计(§3.11 / §3.13,仅 Debug)
// ---------------------------------------------------------------------------

/// §3.11 `perf_snapshot`:性能统计快照。
#[cfg(debug_assertions)]
#[tauri::command]
pub fn perf_snapshot(service: Svc<'_>) -> Result<PerfSnapshot, BeamError> {
    Ok(service.perf_snapshot())
}

/// Release 构建:保留同名命令,但直接返回 `E11003`,
/// 这样前端调用不会 panic,同时 release 产物不含 perf 采集符号(§7 release 无 perf 符号校验)。
#[cfg(not(debug_assertions))]
#[tauri::command]
pub fn perf_snapshot() -> Result<PerfSnapshot, BeamError> {
    Err(BeamError::invalid(
        E_PERF_UNAVAILABLE,
        "性能统计仅在 Debug 构建可用(§3.13 perf-stats 门控)",
    ))
}

/// §3.11 `perf_export(format)`:导出性能数据,`format` 取 `csv` / `json`。
#[cfg(debug_assertions)]
#[tauri::command]
pub fn perf_export(service: Svc<'_>, format: String) -> Result<PerfExportResult, BeamError> {
    service.perf_export(&format)
}

/// Release 构建:见 [`perf_snapshot`] 的同名占位说明。
#[cfg(not(debug_assertions))]
#[tauri::command]
pub fn perf_export(format: String) -> Result<PerfExportResult, BeamError> {
    Err(BeamError::invalid(
        E_PERF_UNAVAILABLE,
        format!("性能统计仅在 Debug 构建可用,无法导出 {format}"),
    ))
}

// ---------------------------------------------------------------------------
// 补充只读 command(§3.11 清单外,「设备」页所需)
// ---------------------------------------------------------------------------

/// 列出已配对设备(§3.3 信任库)。
///
/// §3.11 的 12 个命令里没有读取设备列表的入口,这里补一个纯读取 command,
/// 否则「设备」页无法展示已配对设备。
#[tauri::command]
pub fn list_devices(service: Svc<'_>) -> Vec<Device> {
    service.devices()
}

/// 列出待确认的配对请求。
///
/// 与 [`list_devices`] 同理:§3.11 清单外补充的纯读取 command。
#[tauri::command]
pub fn list_pair_requests(service: Svc<'_>) -> Vec<PairRequest> {
    service.pair_requests()
}
