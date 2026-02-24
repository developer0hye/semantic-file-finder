use std::path::{Path, PathBuf};

use tracing::info;

use crate::config::{self, AppConfig};
use crate::db::Database;
use crate::error::AppError;
use crate::pipeline;
use crate::tantivy_index::TantivyIndex;

/// Application identifier matching `tauri.conf.json` → `identifier`.
pub const APP_IDENTIFIER: &str = "com.semantic-file-search.app";

/// Shared resources initialized at startup, used by both GUI and CLI.
pub struct AppResources {
    pub db: Database,
    pub tantivy: TantivyIndex,
    pub config: AppConfig,
    pub data_dir: PathBuf,
}

/// Resolve the platform-specific data directory for this application.
///
/// - macOS: `~/Library/Application Support/com.semantic-file-search.app`
/// - Linux: `~/.local/share/com.semantic-file-search.app`
/// - Windows: `%AppData%/com.semantic-file-search.app`
pub fn resolve_data_dir() -> Result<PathBuf, AppError> {
    let base = dirs::data_dir().ok_or_else(|| {
        AppError::Internal("failed to resolve platform data directory".to_string())
    })?;
    Ok(base.join(APP_IDENTIFIER))
}

/// Initialize all shared resources (database, Tantivy index, config) at the
/// given data directory. Both the Tauri GUI and CLI binary call this function.
pub fn initialize(data_dir: &Path) -> Result<AppResources, AppError> {
    std::fs::create_dir_all(data_dir).map_err(|e| AppError::FileIo {
        path: data_dir.display().to_string(),
        source: e,
    })?;

    // Database
    let db_path = data_dir.join("index.db");
    let database = Database::open(&db_path)?;

    // Tantivy index
    let tantivy_path = data_dir.join("tantivy_index");
    std::fs::create_dir_all(&tantivy_path).map_err(|e| AppError::FileIo {
        path: tantivy_path.display().to_string(),
        source: e,
    })?;
    let mut tantivy = TantivyIndex::open(&tantivy_path)?;

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

    // Configuration
    let app_config = config::load_config(data_dir).unwrap_or_default();

    Ok(AppResources {
        db: database,
        tantivy,
        config: app_config,
        data_dir: data_dir.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sftest_app_init_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_resolve_data_dir_ends_with_app_identifier() {
        let data_dir = resolve_data_dir().unwrap();
        assert!(
            data_dir.ends_with(APP_IDENTIFIER),
            "data_dir should end with {APP_IDENTIFIER}, got: {}",
            data_dir.display()
        );
    }

    #[test]
    fn test_initialize_opens_db_and_tantivy() {
        let dir = create_test_dir("init_db_tantivy");
        let resources = initialize(&dir).unwrap();

        // DB should be functional
        assert_eq!(resources.db.get_indexed_count().unwrap(), 0);

        // Tantivy should be functional
        let results = resources.tantivy.search("test", 10).unwrap();
        assert!(results.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn test_initialize_loads_default_config() {
        let dir = create_test_dir("init_config");
        let resources = initialize(&dir).unwrap();

        assert_eq!(resources.config, AppConfig::default());
        assert_eq!(resources.data_dir, dir);

        cleanup(&dir);
    }

    #[test]
    fn test_initialize_loads_custom_config() {
        let dir = create_test_dir("init_custom_config");
        let custom_config = AppConfig {
            search_alpha: 0.8,
            ..AppConfig::default()
        };
        config::save_config(&dir, &custom_config).unwrap();

        let resources = initialize(&dir).unwrap();
        assert!((resources.config.search_alpha - 0.8).abs() < f32::EPSILON);

        cleanup(&dir);
    }

    #[test]
    fn test_initialize_creates_data_dir_if_missing() {
        let dir = std::env::temp_dir().join(format!(
            "sftest_app_init_missing_dir_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        assert!(!dir.exists());
        let resources = initialize(&dir).unwrap();
        assert!(dir.exists());
        assert_eq!(resources.db.get_indexed_count().unwrap(), 0);

        cleanup(&dir);
    }
}
