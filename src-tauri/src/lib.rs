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

use tauri::Manager;
use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
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

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, _event| {});
}
