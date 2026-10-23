mod approvals;
mod commands;
mod tray;

use std::sync::{Arc, Mutex};

use plexus_core::{Hub, HubOptions, http::HttpServer, secrets::KeychainSecrets};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

use crate::{approvals::Approvals, commands::ServerStatus};

pub struct AppState {
    pub hub: Hub,
    pub approvals: Approvals,
    server: tokio::sync::Mutex<Option<HttpServer>>,
    server_info: Mutex<ServerStatus>,
}

impl AppState {
    pub fn server_status(&self) -> ServerStatus {
        self.server_info.lock().unwrap().clone()
    }

    pub fn profile_url(&self, slug: &str) -> String {
        format!("http://127.0.0.1:{}/mcp/{slug}", self.server_status().port)
    }
}

pub fn show_dashboard(app: &AppHandle, route: Option<&str>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        if let Some(route) = route {
            let _ = w.emit("navigate", route);
        }
    }
}

/// (Re)bind the HTTP endpoint on the configured port.
pub async fn restart_server(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut server = state.server.lock().await;
    if let Some(old) = server.take() {
        old.shutdown().await;
    }
    let port = state.hub.settings().map(|s| s.port).unwrap_or(7420);
    let status = match plexus_core::http::serve(state.hub.clone(), port).await {
        Ok(s) => {
            tracing::info!("listening on {}", s.addr);
            *server = Some(s);
            ServerStatus {
                running: true,
                port,
                error: None,
            }
        }
        Err(e) => {
            tracing::error!("server failed to start: {e:#}");
            ServerStatus {
                running: false,
                port,
                error: Some(format!("{e:#}")),
            }
        }
    };
    *state.server_info.lock().unwrap() = status.clone();
    let _ = app.emit("server-status", status);
    tray::refresh(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,plexus_core=debug".into()),
        )
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_dashboard(app, None);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            enable_autostart_once(app.handle(), &data_dir);

            let hub = tauri::async_runtime::block_on(Hub::new(HubOptions {
                db_path: Some(data_dir.join("plexus.db")),
                secrets: Arc::new(KeychainSecrets::new("dev.plexus.app")),
                resolve_shell_path: true,
            }))?;
            let approvals = Approvals::new(app.handle().clone());
            hub.set_approval_broker(Arc::new(approvals.clone()));
            let port = hub.settings()?.port;
            app.manage(AppState {
                hub: hub.clone(),
                approvals,
                server: tokio::sync::Mutex::new(None),
                server_info: Mutex::new(ServerStatus {
                    running: false,
                    port,
                    error: None,
                }),
            });

            tray::create(app.handle())?;

            // Forward hub events to the UI and keep the tray menu current.
            let handle = app.handle().clone();
            let mut rx = hub.subscribe();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let _ = handle.emit("hub-event", &event);
                    if !matches!(event, plexus_core::HubEvent::CallLogged { .. }) {
                        tray::refresh(&handle);
                    }
                }
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { restart_server(&handle).await });
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // The dashboard hides instead of quitting; Plexus lives in the menu bar.
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            // Closing an approval popup without choosing counts as a denial.
            WindowEvent::Destroyed => {
                if let Some(id) = window.label().strip_prefix("approval-") {
                    let state = window.state::<AppState>();
                    state
                        .approvals
                        .resolve(id, plexus_core::ApprovalDecision::Deny);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_connections,
            commands::create_connection,
            commands::update_connection,
            commands::delete_connection,
            commands::test_connection,
            commands::stop_connection,
            commands::connection_tools,
            commands::list_profiles,
            commands::create_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::profile_token,
            commands::rotate_profile_token,
            commands::profile_tools,
            commands::set_tool_policy,
            commands::recent_calls,
            commands::get_settings,
            commands::save_settings,
            commands::server_status,
            commands::get_approval,
            commands::resolve_approval,
            commands::start_oauth,
            commands::sign_out,
            commands::install_client,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Plexus");

    app.run(|app, event| match event {
        // Keep running in the menu bar when the last window closes.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => {
            let state = app.state::<AppState>();
            tauri::async_runtime::block_on(state.hub.stop_all());
        }
        _ => {}
    });
}

/// Launch at login is on by default, but only switched on once so a user's
/// later opt-out sticks.
fn enable_autostart_once(app: &AppHandle, data_dir: &std::path::Path) {
    use tauri_plugin_autostart::ManagerExt;
    let marker = data_dir.join(".autostart-initialized");
    if marker.exists() || cfg!(debug_assertions) {
        return;
    }
    if app.autolaunch().enable().is_ok() {
        let _ = std::fs::write(marker, "");
    }
}
