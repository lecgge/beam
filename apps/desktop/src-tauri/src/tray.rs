//! §3.11 托盘:菜单 + 一键开播/停止。
//!
//! 托盘菜单与主窗口共享同一套服务状态,
//! 「▶ 一键开播」等价于 `start_hosting(opts)`、「■ 停止串流」等价于 `stop_hosting`,
//! 状态变化通过 `host://state-changed` 事件同步回前端。

use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::dto::{HostState, HostStatus, StartHostingOptions};
use crate::events;
use crate::service::BeamService;

/// 托盘图标 id(与 §3.11 的托盘职责对应)。
pub const TRAY_ID: &str = "beam-tray";

/// 托盘菜单项句柄:用于按主机状态启用/禁用「开播 / 停止」。
pub struct TrayMenu {
    pub start: MenuItem<tauri::Wry>,
    pub stop: MenuItem<tauri::Wry>,
    pub show: MenuItem<tauri::Wry>,
    pub quit: MenuItem<tauri::Wry>,
}

/// 创建托盘图标与菜单。
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let start = MenuItemBuilder::with_id("tray_start", "▶ 一键开播").build(app)?;
    let stop = MenuItemBuilder::with_id("tray_stop", "■ 停止串流").build(app)?;
    let show = MenuItemBuilder::with_id("tray_show", "显示主窗口").build(app)?;
    let quit = MenuItemBuilder::with_id("tray_quit", "退出 Beam").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&start)
        .item(&stop)
        .separator()
        .item(&show)
        .item(&quit)
        .build()?;

    // 图标优先用窗口默认图标;取不到时退回内置的 32x32 PNG。
    let icon = match app.default_window_icon().cloned() {
        Some(icon) => icon,
        None => tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?,
    };

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Beam 串流")
        .build(app)?;

    app.manage(TrayMenu {
        start,
        stop,
        show,
        quit,
    });
    Ok(())
}

/// 按主机状态同步托盘菜单(可用性与提示文案)。
pub fn sync_menu_state(app: &AppHandle, status: &HostStatus) {
    let Some(menu) = app.try_state::<TrayMenu>() else {
        return;
    };
    let hosting = matches!(status.state, HostState::Hosting | HostState::Streaming);
    let _ = menu.start.set_enabled(!hosting);
    let _ = menu.stop.set_enabled(hosting);
    let _ = menu.show.set_enabled(true);
    let _ = menu.quit.set_enabled(true);
}

/// 处理托盘菜单点击。
pub fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let service = app.state::<std::sync::Arc<dyn BeamService>>();
    match event.id().as_ref() {
        "tray_start" => match service.start_hosting(StartHostingOptions::default()) {
            Ok(status) => {
                events::emit(app, events::HOST_STATE_CHANGED, status.clone());
                sync_menu_state(app, &status);
            }
            Err(err) => {
                tracing::warn!(code = %err.code, error = %err.message, "托盘开播失败");
                service.set_host_error(err.message.clone());
                let status = service.host_status();
                events::emit(app, events::HOST_STATE_CHANGED, status.clone());
                sync_menu_state(app, &status);
            }
        },
        "tray_stop" => match service.stop_hosting() {
            Ok(status) => {
                events::emit(app, events::HOST_STATE_CHANGED, status.clone());
                sync_menu_state(app, &status);
            }
            Err(err) => {
                tracing::warn!(code = %err.code, error = %err.message, "托盘停止失败");
                service.set_host_error(err.message.clone());
                let status = service.host_status();
                events::emit(app, events::HOST_STATE_CHANGED, status.clone());
                sync_menu_state(app, &status);
            }
        },
        "tray_show" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "tray_quit" => app.exit(0),
        _ => {}
    }
}
