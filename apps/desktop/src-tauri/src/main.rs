//! Beam 串流 Windows 桌面端(Tauri 2 外壳)。
//!
//! §2 工程结构:`apps/desktop/src-tauri/` 下的 `main.rs`、`commands.rs`、`events.rs`、`tray.rs`。
//! §3.11 职责:UI、托盘、通知;只通过 commands/events 与 Rust 交互,视频帧不经 IPC。
//!
//! 生命周期:
//! - `setup`:建托盘、起后台事件定时器、初始化托盘菜单状态。
//! - `on_menu_event`:托盘菜单(§3.11 一键开播 / 停止)。
//! - `on_window_event`:关闭主窗口 = 收进托盘(进程存活);彻底退出走托盘「退出 Beam」。
//! - `invoke_handler`:注册 §3.11 的 12 个 command + Debug 下的 perf 命令。

// 错误码与 DTO 字段是完整目录,部分常量/字段只被前端或未来的 crates/ 使用,
// 在二进制 crate 里会触发 dead_code 噪音,这里统一放开。
#![allow(dead_code)]

mod commands;
mod dto;
mod error;
mod events;
// §3.3 的 PIN 格式校验与尝试预算(S11):`connect` 的 PIN 参数在这里被真正校验。
mod pin;
mod service;
// crates/ 适配器(§4:apps/desktop → beam-host / beam-client)。
// 关闭 `beam-crates` feature 时退回 service.rs 的本地桩实现。
#[cfg(feature = "beam-crates")]
mod service_crates;
#[cfg(feature = "beam-crates")]
mod transport;
mod tray;

use std::sync::Arc;

use tauri::Manager;

use crate::service::BeamService;

fn main() {
    // §3.15:tracing 初始化;热路径只允许 trace! 且默认关闭。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 服务层:默认走 crates/ 真实实现;关闭 feature 时是本地内存桩(见 service.rs)。
    let service: Arc<dyn BeamService> = service::create_service();

    tauri::Builder::default()
        .manage(service.clone())
        .setup(move |app| {
            tray::setup(app.handle())?;
            events::start_background_timers(app.handle().clone(), service.clone());

            // 初始托盘菜单状态:待机时只能「开播」。
            let status = app.state::<Arc<dyn BeamService>>().host_status();
            tray::sync_menu_state(app.handle(), &status);
            Ok(())
        })
        .on_menu_event(tray::handle_menu_event)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // §3.11:主窗口关闭后收进托盘,托盘菜单继续提供「显示主窗口 / 退出」。
                let minimize_to_tray = window
                    .state::<Arc<dyn BeamService>>()
                    .settings()
                    .minimize_to_tray;
                if minimize_to_tray {
                    api.prevent_close();
                    if let Some(w) = window.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // §3.11 主机
            commands::get_host_status,
            commands::start_hosting,
            commands::stop_hosting,
            // §3.11 配对
            commands::get_pairing_code,
            commands::approve_pair,
            commands::reject_pair,
            commands::remove_device,
            // §3.11 发现与连接
            commands::list_hosts,
            commands::connect,
            commands::disconnect,
            // §3.11 设置
            commands::get_settings,
            commands::set_settings,
            // §3.11 Debug 专属
            commands::perf_snapshot,
            commands::perf_export,
            // 补充:「设备」页所需的只读入口
            commands::list_devices,
            commands::list_pair_requests,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Beam 桌面端失败");
}
