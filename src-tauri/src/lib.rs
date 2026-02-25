pub mod app_init;
pub mod commands;
pub mod config;
pub mod converter;
pub mod crawler;
pub mod db;
pub mod error;
pub mod gemini;
pub mod keychain;
pub mod logging;
pub mod pipeline;
pub mod platform;
pub mod search;
pub mod tantivy_index;
pub mod vector_search;
pub mod watcher;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tauri::Manager;
use tokio::sync::Mutex;
use tracing::info;

use commands::AppState;
use pipeline::{IndexingState, IndexingStatus};
use watcher::FileWatcher;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app_data_dir");

            let log_dir = app_data_dir.join("logs");
            std::fs::create_dir_all(&log_dir).expect("failed to create log directory");

            // The guard must be kept alive for the lifetime of the app.
            // Leaking it is intentional — it will be cleaned up on process exit.
            let guard = logging::init_logging(&log_dir);
            std::mem::forget(guard);

            info!(
                app_data_dir = %app_data_dir.display(),
                "application started"
            );

            let resources = app_init::initialize(&app_data_dir)
                .expect("failed to initialize application resources");

            // Try to load API key and create Gemini client
            let gemini_client = keychain::get_api_key().ok().flatten().map(|key| {
                gemini::GeminiClient::new(
                    key,
                    resources.config.gemini_model.clone(),
                    resources.config.embedding_model.clone(),
                    resources.config.embedding_dimensions,
                )
            });

            let db = Arc::new(Mutex::new(resources.db));
            let tantivy = Arc::new(Mutex::new(resources.tantivy));
            let gemini = Arc::new(Mutex::new(gemini_client));

            let exclude_dirs: Vec<String> = platform::default_exclude_dirs()
                .into_iter()
                .map(|s| s.to_string())
                .collect();

            let file_watcher = match FileWatcher::start(
                resources.config.watch_directories.clone(),
                resources.config.supported_extensions.clone(),
                exclude_dirs,
                db.clone(),
                tantivy.clone(),
                gemini.clone(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to start file watcher on startup");
                    None
                }
            };

            let state = AppState {
                db,
                tantivy,
                gemini,
                config: Arc::new(Mutex::new(resources.config)),
                data_dir: resources.data_dir,
                indexing_status: Arc::new(Mutex::new(IndexingStatus {
                    state: IndexingState::Idle,
                    total_files: 0,
                    indexed_files: 0,
                    failed_files: 0,
                    current_file: None,
                })),
                pause_flag: Arc::new(AtomicBool::new(false)),
                watcher: Arc::new(Mutex::new(file_watcher)),
            };

            app.manage(state);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::search_files,
            commands::start_indexing,
            commands::pause_indexing,
            commands::resume_indexing,
            commands::get_indexing_status,
            commands::validate_api_key,
            commands::save_api_key,
            commands::get_config,
            commands::update_config,
            commands::open_file,
            commands::get_indexed_stats,
            commands::get_all_supported_extensions,
            commands::get_watcher_status,
            commands::restart_watcher,
            commands::stop_watcher,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, _event| {});
}
