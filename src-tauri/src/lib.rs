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

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tauri::Manager;
use tokio::sync::Mutex;
use tracing::info;

use commands::AppState;
use pipeline::{IndexingState, IndexingStatus};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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

            // Initialize database
            let db_path = app_data_dir.join("index.db");
            let database = db::Database::open(&db_path).expect("failed to open database");

            // Initialize Tantivy index
            let tantivy_path = app_data_dir.join("tantivy_index");
            std::fs::create_dir_all(&tantivy_path)
                .expect("failed to create tantivy_index directory");
            let mut tantivy = tantivy_index::TantivyIndex::open(&tantivy_path)
                .expect("failed to open Tantivy index");

            // Reconcile any pending_tantivy records from previous interrupted indexing
            match pipeline::reconcile_pending_tantivy(&database, &mut tantivy) {
                Ok(count) if count > 0 => {
                    info!(reconciled = count, "reconciled pending_tantivy records");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to reconcile pending_tantivy");
                }
                _ => {}
            }

            // Load configuration
            let app_config = config::load_config(&app_data_dir).unwrap_or_default();

            // Try to load API key and create Gemini client
            let gemini_client = keychain::get_api_key().ok().flatten().map(|key| {
                gemini::GeminiClient::new(
                    key,
                    app_config.gemini_model.clone(),
                    app_config.embedding_model.clone(),
                    app_config.embedding_dimensions,
                )
            });

            let state = AppState {
                db: Arc::new(Mutex::new(database)),
                tantivy: Arc::new(Mutex::new(tantivy)),
                gemini: Arc::new(Mutex::new(gemini_client)),
                config: Arc::new(Mutex::new(app_config)),
                data_dir: app_data_dir,
                indexing_status: Arc::new(Mutex::new(IndexingStatus {
                    state: IndexingState::Idle,
                    total_files: 0,
                    indexed_files: 0,
                    failed_files: 0,
                    current_file: None,
                })),
                pause_flag: Arc::new(AtomicBool::new(false)),
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, _event| {});
}
