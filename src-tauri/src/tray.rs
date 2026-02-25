use std::sync::Arc;
use std::sync::atomic::Ordering;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager};
use tracing::{info, warn};

use crate::commands::AppState;
use crate::crawler::crawl_directory;
use crate::error::AppError;
use crate::gemini::GeminiClient;
use crate::pipeline;
use crate::platform::default_exclude_dirs;

const MENU_ID_SHOW_WINDOW: &str = "show_window";
const MENU_ID_START_INDEXING: &str = "start_indexing";
const MENU_ID_PAUSE_INDEXING: &str = "pause_indexing";
const MENU_ID_RESUME_INDEXING: &str = "resume_indexing";
const MENU_ID_QUIT: &str = "quit";

/// Creates the system tray icon with a context menu and registers event handlers.
pub fn setup_tray(app: &App) -> Result<(), AppError> {
    let menu = build_tray_menu(app)?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder
        .build(app)
        .map_err(|e| AppError::Internal(format!("failed to create tray icon: {e}")))?;

    info!("system tray initialized");
    Ok(())
}

/// Builds the tray context menu.
fn build_tray_menu(app: &App) -> Result<Menu<tauri::Wry>, AppError> {
    let map_err = |e: tauri::Error| AppError::Internal(format!("failed to build tray menu: {e}"));

    let show = MenuItem::with_id(app, MENU_ID_SHOW_WINDOW, "Show Window", true, None::<&str>)
        .map_err(map_err)?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(map_err)?;
    let start = MenuItem::with_id(
        app,
        MENU_ID_START_INDEXING,
        "Start Indexing",
        true,
        None::<&str>,
    )
    .map_err(map_err)?;
    let pause = MenuItem::with_id(
        app,
        MENU_ID_PAUSE_INDEXING,
        "Pause Indexing",
        true,
        None::<&str>,
    )
    .map_err(map_err)?;
    let resume = MenuItem::with_id(
        app,
        MENU_ID_RESUME_INDEXING,
        "Resume Indexing",
        true,
        None::<&str>,
    )
    .map_err(map_err)?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(map_err)?;
    let quit = MenuItem::with_id(app, MENU_ID_QUIT, "Quit", true, None::<&str>).map_err(map_err)?;

    Menu::with_items(app, &[&show, &sep1, &start, &pause, &resume, &sep2, &quit])
        .map_err(|e| AppError::Internal(format!("failed to build tray menu: {e}")))
}

/// Dispatches tray menu item clicks to the appropriate action.
fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        MENU_ID_SHOW_WINDOW => show_main_window(app),
        MENU_ID_START_INDEXING => trigger_indexing(app),
        MENU_ID_PAUSE_INDEXING => pause_indexing(app),
        MENU_ID_RESUME_INDEXING => resume_indexing(app),
        MENU_ID_QUIT => app.exit(0),
        other => warn!(menu_id = other, "unknown tray menu item clicked"),
    }
}

/// Shows and focuses the main application window.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Triggers indexing on configured watch directories.
fn trigger_indexing(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();

        let config = state.config.lock().await;
        let directories = config.watch_directories.clone();
        let supported_extensions = config.supported_extensions.clone();
        drop(config);

        if directories.is_empty() {
            warn!("no watch directories configured for indexing");
            return;
        }

        let exclude_dirs = default_exclude_dirs();
        let mut all_entries = Vec::new();
        for dir in &directories {
            let entries = crawl_directory(
                std::path::Path::new(dir),
                &exclude_dirs,
                &supported_extensions,
            );
            all_entries.extend(entries);
        }

        info!(
            total_files = all_entries.len(),
            directories = ?directories,
            "tray: starting indexing"
        );

        let gemini_guard = state.gemini.lock().await;
        let gemini: Option<Arc<GeminiClient>> = gemini_guard.as_ref().map(|client| {
            Arc::new(GeminiClient::new(
                client.api_key().to_string(),
                client.model().to_string(),
                client.embedding_model().to_string(),
                client.embedding_dimensions(),
            ))
        });
        drop(gemini_guard);

        let db = state.db.clone();
        let tantivy = state.tantivy.clone();
        let status = state.indexing_status.clone();
        let pause_flag = state.pause_flag.clone();

        if let Err(e) =
            pipeline::run_pipeline(gemini, db, tantivy, all_entries, 5, status, pause_flag).await
        {
            tracing::error!(error = %e, "tray: indexing pipeline failed");
        }
    });
}

/// Pauses the indexing pipeline.
fn pause_indexing(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.pause_flag.store(true, Ordering::Relaxed);
    info!("tray: indexing paused");
}

/// Resumes the indexing pipeline.
fn resume_indexing(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.pause_flag.store(false, Ordering::Relaxed);
    info!("tray: indexing resumed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const ALL_MENU_IDS: &[&str] = &[
        MENU_ID_SHOW_WINDOW,
        MENU_ID_START_INDEXING,
        MENU_ID_PAUSE_INDEXING,
        MENU_ID_RESUME_INDEXING,
        MENU_ID_QUIT,
    ];

    #[test]
    fn test_menu_item_ids_are_unique() {
        let mut seen = HashSet::new();
        for id in ALL_MENU_IDS {
            assert!(seen.insert(*id), "duplicate menu item ID: {id}");
        }
    }

    #[test]
    fn test_menu_item_ids_are_nonempty() {
        for id in ALL_MENU_IDS {
            assert!(!id.is_empty(), "menu item ID must not be empty");
        }
    }

    #[test]
    fn test_all_menu_ids_contains_expected_items() {
        assert_eq!(ALL_MENU_IDS.len(), 5);
        assert!(ALL_MENU_IDS.contains(&"show_window"));
        assert!(ALL_MENU_IDS.contains(&"start_indexing"));
        assert!(ALL_MENU_IDS.contains(&"pause_indexing"));
        assert!(ALL_MENU_IDS.contains(&"resume_indexing"));
        assert!(ALL_MENU_IDS.contains(&"quit"));
    }
}
