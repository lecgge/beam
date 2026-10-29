//! §3.11 的 8 个事件发射点。
//!
//! | 事件 | 载荷 | 频率 |
//! |---|---|---|
//! | `host://state-changed` | [`HostStatus`] | 状态变化时 |
//! | `pair://request` | [`PairRequest`] | 收到新配对请求时 |
//! | `pair://code-rotated` | [`PairingCode`] | PIN 轮换时(§3.3 `pin_ttl=120s`) |
//! | `discovery://updated` | [`Vec<DiscoveredHost>`] | 发现列表刷新时 |
//! | `session://stats` | [`SessionStats`] | 500 ms |
//! | `session://reconnecting` | [`ReconnectingInfo`] | 进入重连时 |
//! | `session://disconnected` | [`DisconnectedInfo`] | 会话结束时 |
//! | `perf://tick` | [`PerfTick`] | 250 ms,仅 Debug(§3.13) |

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::dto::DisconnectedInfo;
use crate::service::{BeamService, SessionEventInfo};

// ---- 事件名常量 ----
pub const HOST_STATE_CHANGED: &str = "host://state-changed";
pub const PAIR_REQUEST: &str = "pair://request";
pub const PAIR_CODE_ROTATED: &str = "pair://code-rotated";
pub const DISCOVERY_UPDATED: &str = "discovery://updated";
pub const SESSION_STATS: &str = "session://stats";
pub const SESSION_RECONNECTING: &str = "session://reconnecting";
pub const SESSION_DISCONNECTED: &str = "session://disconnected";
pub const PERF_TICK: &str = "perf://tick";

/// §3.11:`session://stats` 500 ms 一拍。
pub const SESSION_STATS_INTERVAL_MS: u64 = 500;
/// §3.11:`perf://tick` 250 ms 一拍(仅 Debug)。
pub const PERF_TICK_INTERVAL_MS: u64 = 250;
/// PIN 轮换检查间隔。
pub const PAIR_CODE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// 发现列表刷新间隔。
pub const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);

/// 统一的发射封装:失败只记日志,不打断业务流程。
pub fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(err) = app.emit(event, payload) {
        tracing::warn!(event, error = %err, "事件发射失败");
    }
}

/// 启动后台定时器,驱动周期性与状态型事件。
pub fn start_background_timers(app: AppHandle, service: Arc<dyn BeamService>) {
    // --- session://stats(500 ms)---
    {
        let app = app.clone();
        let service = service.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(SESSION_STATS_INTERVAL_MS)).await;
                // §3.9:进入重连状态时单独发一次 session://reconnecting。
                if let Some(notice) = service.reconnect_notice() {
                    emit(&app, SESSION_RECONNECTING, notice);
                }
                // §3.2:真实传输层的握手结果。失败按「会话结束」上报,
                // 成功时服务层已把会话状态推进到 streaming。
                if let Some(outcome) = service.take_transport_outcome() {
                    if !outcome.ok {
                        let session_id =
                            service.session().map(|s| s.session_id).unwrap_or_default();
                        emit(
                            &app,
                            SESSION_DISCONNECTED,
                            DisconnectedInfo {
                                session_id,
                                reason: "transport_failed".to_string(),
                                code: outcome.code,
                            },
                        );
                    }
                }
                // §3.9:真实客户端会话的事件(协商完成 / 出错 / 结束)。
                for event in service.take_session_events() {
                    match event {
                        SessionEventInfo::Negotiated { .. } => {
                            // 会话状态已经同步到服务层,广播主机状态让前端刷新。
                            emit(&app, HOST_STATE_CHANGED, service.host_status());
                        }
                        SessionEventInfo::Error { code, retryable } => {
                            let session_id =
                                service.session().map(|s| s.session_id).unwrap_or_default();
                            emit(
                                &app,
                                SESSION_DISCONNECTED,
                                DisconnectedInfo {
                                    session_id,
                                    reason: "session_error".to_string(),
                                    code: Some(format!("E{code}")),
                                },
                            );
                            if retryable {
                                tracing::warn!(code, "客户端会话出错(可重试)");
                            } else {
                                tracing::error!(code, "客户端会话出错");
                            }
                        }
                        SessionEventInfo::Closed => {
                            let session_id =
                                service.session().map(|s| s.session_id).unwrap_or_default();
                            emit(
                                &app,
                                SESSION_DISCONNECTED,
                                DisconnectedInfo {
                                    session_id,
                                    reason: "closed".to_string(),
                                    code: None,
                                },
                            );
                        }
                        SessionEventInfo::Paired { fingerprint } => {
                            tracing::info!(%fingerprint, "配对完成");
                        }
                        SessionEventInfo::Stats { .. } => {}
                    }
                }
                // §3.3:新出现的配对请求(主机端人工确认)。
                for request in service.take_new_pair_requests() {
                    emit(&app, PAIR_REQUEST, request);
                }
                if let Some(stats) = service.session_stats() {
                    emit(&app, SESSION_STATS, stats);
                }
            }
        });
    }

    // --- pair://code-rotated(PIN 过期轮换,§3.3)---
    {
        let app = app.clone();
        let service = service.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(PAIR_CODE_CHECK_INTERVAL).await;
                if let Some(code) = service.rotate_pairing_code_if_due() {
                    emit(&app, PAIR_CODE_ROTATED, code);
                }
            }
        });
    }

    // --- discovery://updated(发现列表刷新,§3.4)---
    {
        let app = app.clone();
        let service = service.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(DISCOVERY_INTERVAL).await;
                emit(&app, DISCOVERY_UPDATED, service.list_hosts());
            }
        });
    }

    // --- perf://tick(250 ms,仅 Debug 构建;§3.13 perf-stats 门控)---
    #[cfg(debug_assertions)]
    {
        let app = app.clone();
        let service = service.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(PERF_TICK_INTERVAL_MS)).await;
                if let Some(tick) = service.perf_tick() {
                    emit(&app, PERF_TICK, tick);
                }
            }
        });
    }

    // --- pair://request(启动后把待确认的配对请求推给前端)---
    {
        let app = app.clone();
        let service = service.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            for request in service.pair_requests() {
                emit(&app, PAIR_REQUEST, request);
            }
        });
    }
}
