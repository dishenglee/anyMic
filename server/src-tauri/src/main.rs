#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use anymic_tauri::{start_server, LiveStats, ServerConfig, ServerHandle};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

// ── App state ─────────────────────────────────────────────────────────────────

struct AppState {
    handle: Mutex<Option<ServerHandle>>,
    rt: tokio::runtime::Runtime,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Start the server.  Returns the initial LiveStats on success.
#[tauri::command]
fn start(state: tauri::State<'_, AppState>) -> Result<LiveStats, String> {
    let mut guard = state.handle.lock().unwrap();
    if guard.is_some() {
        return Err("Server is already running".to_string());
    }

    let cfg = ServerConfig::default();
    let handle = state
        .rt
        .block_on(start_server(cfg))
        .map_err(|e| e.to_string())?;

    let stats = {
        let s = handle.stats();
        let locked = s.lock();
        locked.clone()
    };

    *guard = Some(handle);
    Ok(stats)
}

/// Stop the server.
#[tauri::command]
fn stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let handle = state.handle.lock().unwrap().take();
    if let Some(h) = handle {
        state.rt.block_on(h.stop()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Get the current live stats.
#[tauri::command]
fn get_stats(state: tauri::State<'_, AppState>) -> LiveStats {
    let guard = state.handle.lock().unwrap();
    if let Some(ref h) = *guard {
        let s = h.stats();
        let locked = s.lock();
        locked.clone()
    } else {
        LiveStats::default()
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    // Auto-start the server when the app launches (dev convenience).
    // The UI start/stop buttons also work for interactive control.
    let app_state = AppState {
        handle: Mutex::new(None),
        rt,
    };

    // Pre-start the server so it's ready even before the user clicks Start.
    {
        let cfg = ServerConfig::default();
        match app_state.rt.block_on(start_server(cfg)) {
            Ok(handle) => {
                tracing::info!("auto-started server");
                *app_state.handle.lock().unwrap() = Some(handle);
            }
            Err(e) => {
                tracing::warn!("auto-start failed: {e}");
            }
        }
    }

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![start, stop, get_stats])
        .setup(|app| {
            // System tray with bilingual menu items.
            let show = MenuItem::with_id(app, "show", "显示窗口 / Show", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 / Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("anyMic")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
