//! Menu bar icon and its menu, rebuilt whenever hub state changes.

use plexus_core::model::{ConnectionStatus, TunnelStatus};
use tauri::{
    AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::{AppState, show_dashboard};

const TRAY_ID: &str = "plexus";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Plexus")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event)
        .build(app)?;
    Ok(())
}

pub fn refresh(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id(TRAY_ID) else {
            return;
        };
        match build_menu(&handle) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(e) => tracing::warn!("tray menu rebuild failed: {e}"),
        }
    });
}

fn status_label(status: &ConnectionStatus) -> (&'static str, &'static str) {
    match status {
        ConnectionStatus::Running => ("🟢", "running"),
        ConnectionStatus::Idle => ("⚪️", "idle"),
        ConnectionStatus::Starting => ("🟡", "starting"),
        ConnectionStatus::NeedsAuth => ("🔑", "needs sign-in"),
        ConnectionStatus::Error { .. } => ("🔴", "error"),
        ConnectionStatus::Disabled => ("⏸", "disabled"),
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let server = state.server_status();
    let menu = Menu::new(app)?;

    let header = match (&server.running, &server.error) {
        (true, _) => format!("Plexus · listening on :{}", server.port),
        (false, Some(_)) => format!("Plexus · port {} unavailable", server.port),
        (false, None) => "Plexus · starting…".to_string(),
    };
    menu.append(&MenuItem::with_id(app, "header", header, false, None::<&str>)?)?;
    let tunnel = state.hub.tunnel_status();
    let (tunnel_label, tunnel_toggle) = match &tunnel {
        TunnelStatus::Off => ("Tunnel off".to_string(), "Start Tunnel"),
        TunnelStatus::Starting => ("Tunnel starting…".to_string(), "Stop Tunnel"),
        TunnelStatus::Running { url } => (
            format!("Tunnel · {}", url.trim_start_matches("https://")),
            "Stop Tunnel",
        ),
        TunnelStatus::Error { .. } => ("Tunnel failed".to_string(), "Retry Tunnel"),
    };
    menu.append(&MenuItem::with_id(app, "tunnel-status", tunnel_label, false, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "tunnel", tunnel_toggle, true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let connections = state.hub.list_connections().unwrap_or_default();
    if connections.is_empty() {
        menu.append(&MenuItem::with_id(
            app,
            "none",
            "No connections yet",
            false,
            None::<&str>,
        )?)?;
    }
    for c in &connections {
        let (dot, label) = status_label(&state.hub.status(&c.id));
        menu.append(&MenuItem::with_id(
            app,
            format!("conn:{}", c.id),
            format!("{dot}  {}  —  {label}", c.name),
            true,
            None::<&str>,
        )?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    if matches!(tunnel, TunnelStatus::Running { .. }) {
        let copy = Submenu::new(app, "Copy Endpoint URL", true)?;
        copy.append(&MenuItem::with_id(app, "copy", "Local", true, None::<&str>)?)?;
        copy.append(&MenuItem::with_id(app, "copy-public", "Public", true, None::<&str>)?)?;
        menu.append(&copy)?;
    } else {
        menu.append(&MenuItem::with_id(
            app,
            "copy",
            "Copy Endpoint URL",
            true,
            None::<&str>,
        )?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        "add",
        "Add Connection…",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open Dashboard…",
        true,
        Some("CmdOrCtrl+,"),
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        "Quit Plexus",
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;
    Ok(menu)
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        "open" => show_dashboard(app, None),
        "add" => show_dashboard(app, Some("/connections/new")),
        "quit" => app.exit(0),
        "copy" => {
            let url = app.state::<AppState>().endpoint_url();
            let _ = app.clipboard().write_text(url);
        }
        "copy-public" => {
            if let Some(base) = app.state::<AppState>().hub.public_url() {
                let _ = app.clipboard().write_text(format!("{base}/mcp"));
            }
        }
        "tunnel" => {
            let app = app.clone();
            let enable = !matches!(
                app.state::<AppState>().hub.tunnel_status(),
                TunnelStatus::Running { .. } | TunnelStatus::Starting
            );
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::set_tunnel(&app, enable).await {
                    tracing::warn!("tunnel: {e:#}");
                }
            });
        }
        _ => {
            if let Some(conn) = id.strip_prefix("conn:") {
                show_dashboard(app, Some(&format!("/connections/{conn}")));
            }
        }
    }
}
