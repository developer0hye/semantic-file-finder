use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::Mutex;
use tracing::info;

use crate::config::{self, AppConfig};
use crate::crawler::{self, crawl_directory};
use crate::db::Database;
use crate::error::AppError;
use crate::gemini::GeminiClient;
use crate::keychain;
use crate::pipeline::{self, IndexingStatus};
use crate::platform::default_exclude_dirs;
use crate::search::{self, SearchMode};
use crate::tantivy_index::{SearchFilters, TantivyIndex};
use crate::watcher::FileWatcher;

/// Shared application state managed by Tauri.
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub tantivy: Arc<Mutex<TantivyIndex>>,
    pub gemini: Arc<Mutex<Option<GeminiClient>>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub data_dir: PathBuf,
    pub indexing_status: Arc<Mutex<IndexingStatus>>,
    pub pause_flag: Arc<AtomicBool>,
    pub watcher: Arc<Mutex<Option<FileWatcher>>>,
}

/// Status of the file system watcher.
#[derive(Debug, Serialize)]
pub struct WatcherStatus {
    pub is_running: bool,
    pub watched_directories: Vec<String>,
}

/// Search response returned to the frontend.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResultItem>,
    pub mode_used: String,
    pub query_time_ms: u64,
}

/// Individual search result item.
#[derive(Debug, Serialize)]
pub struct SearchResultItem {
    pub file_path: String,
    pub file_name: String,
    pub summary: String,
    pub keywords: String,
    pub final_score: f32,
    pub keyword_score: f32,
    pub vector_score: f32,
}

/// Indexed file statistics.
#[derive(Debug, Serialize)]
pub struct IndexedStats {
    pub total_files: usize,
    pub by_extension: HashMap<String, usize>,
    pub total_size_bytes: u64,
}

/// Search mode from the frontend.
#[derive(Debug, Deserialize)]
pub enum FrontendSearchMode {
    Hybrid,
    KeywordOnly,
    VectorOnly,
}

/// Search filters from the frontend.
#[derive(Debug, Deserialize)]
pub struct FrontendSearchFilters {
    /// File extensions to filter by (without dots, e.g., ["pdf", "docx"]).
    pub file_types: Option<Vec<String>>,
    /// Only include files modified at or after this Unix timestamp.
    pub date_after: Option<i64>,
    /// Only include files modified at or before this Unix timestamp.
    pub date_before: Option<i64>,
    /// Only include files under these directory prefixes.
    pub directories: Option<Vec<String>>,
}

impl FrontendSearchFilters {
    fn into_search_filters(self) -> SearchFilters {
        SearchFilters {
            file_extensions: self.file_types.unwrap_or_default(),
            date_after: self.date_after,
            date_before: self.date_before,
            directories: self.directories.unwrap_or_default(),
        }
    }
}

// === Search ===

#[tauri::command]
pub async fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
    mode: Option<FrontendSearchMode>,
    alpha: Option<f32>,
    filters: Option<FrontendSearchFilters>,
) -> Result<SearchResponse, AppError> {
    let start = std::time::Instant::now();
    let limit = limit.unwrap_or(20);
    let alpha = alpha.unwrap_or(0.4);
    let search_filters = filters
        .map(FrontendSearchFilters::into_search_filters)
        .unwrap_or_default();

    let search_mode = match mode {
        Some(FrontendSearchMode::KeywordOnly) => SearchMode::KeywordOnly,
        Some(FrontendSearchMode::VectorOnly) => SearchMode::VectorOnly,
        _ => SearchMode::Hybrid,
    };

    // Get query embedding if needed
    let query_embedding = if search_mode != SearchMode::KeywordOnly {
        let gemini_guard = state.gemini.lock().await;
        if let Some(client) = gemini_guard.as_ref() {
            client.embed_text(&query, "RETRIEVAL_QUERY").await.ok()
        } else {
            None
        }
    } else {
        None
    };

    let db_guard = state.db.lock().await;
    let tantivy_guard = state.tantivy.lock().await;

    // Pre-filter embeddings using DB-level filters when active
    let document_embeddings = if search_filters.has_any_filter() {
        let valid_ids = db_guard.get_filtered_db_ids(&search_filters)?;
        db_guard
            .get_all_embeddings()?
            .into_iter()
            .filter(|(id, _, _, _)| valid_ids.contains(id))
            .collect()
    } else {
        db_guard.get_all_embeddings()?
    };

    let mode_used = if query_embedding.is_some() && search_mode != SearchMode::KeywordOnly {
        search_mode
    } else {
        SearchMode::KeywordOnly
    };

    let params = search::SearchParams {
        mode: mode_used,
        alpha,
        limit,
        filters: &search_filters,
    };
    let results = search::hybrid_search(
        &tantivy_guard,
        &query,
        query_embedding.as_deref(),
        &document_embeddings,
        &params,
    )?;

    let mode_str = match mode_used {
        SearchMode::Hybrid => "Hybrid",
        SearchMode::KeywordOnly => "KeywordOnly",
        SearchMode::VectorOnly => "VectorOnly",
    };

    let items: Vec<SearchResultItem> = results
        .into_iter()
        .map(|r| SearchResultItem {
            file_path: r.file_path,
            file_name: r.file_name,
            summary: r.summary,
            keywords: r.keywords,
            final_score: r.final_score,
            keyword_score: r.keyword_score,
            vector_score: r.vector_score,
        })
        .collect();

    Ok(SearchResponse {
        results: items,
        mode_used: mode_str.to_string(),
        query_time_ms: start.elapsed().as_millis() as u64,
    })
}

// === Indexing ===

#[tauri::command]
pub async fn start_indexing(
    state: State<'_, AppState>,
    directories: Vec<String>,
) -> Result<(), AppError> {
    let config = state.config.lock().await;
    let exclude_dirs = default_exclude_dirs();
    let supported_extensions = config.supported_extensions.clone();

    // Crawl all directories
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
        "starting indexing"
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

    if gemini.is_some() {
        info!("indexing in full mode (Gemini available)");
    } else {
        info!("indexing in keyword-only mode (no Gemini API key)");
    }

    let db = state.db.clone();
    let tantivy = state.tantivy.clone();
    let status = state.indexing_status.clone();
    let pause_flag = state.pause_flag.clone();

    // Run pipeline in background
    tokio::spawn(async move {
        if let Err(e) =
            pipeline::run_pipeline(gemini, db, tantivy, all_entries, 5, status, pause_flag).await
        {
            tracing::error!(error = %e, "indexing pipeline failed");
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn pause_indexing(state: State<'_, AppState>) -> Result<(), AppError> {
    state.pause_flag.store(true, Ordering::Relaxed);
    info!("indexing paused");
    Ok(())
}

#[tauri::command]
pub async fn resume_indexing(state: State<'_, AppState>) -> Result<(), AppError> {
    state.pause_flag.store(false, Ordering::Relaxed);
    info!("indexing resumed");
    Ok(())
}

#[tauri::command]
pub async fn get_indexing_status(state: State<'_, AppState>) -> Result<IndexingStatus, AppError> {
    let status = state.indexing_status.lock().await;
    Ok(status.clone())
}

// === Settings ===

#[tauri::command]
pub async fn validate_api_key(key: String) -> Result<bool, AppError> {
    let client = GeminiClient::new(
        key,
        "gemini-3-flash-preview".into(),
        "gemini-embedding-001".into(),
        1536,
    );
    client.validate_api_key().await
}

#[tauri::command]
pub async fn save_api_key(state: State<'_, AppState>, key: String) -> Result<(), AppError> {
    keychain::store_api_key(&key)?;

    // Update the Gemini client with the new key
    let config = state.config.lock().await;
    let client = GeminiClient::new(
        key,
        config.gemini_model.clone(),
        config.embedding_model.clone(),
        config.embedding_dimensions,
    );
    let mut gemini = state.gemini.lock().await;
    *gemini = Some(client);

    info!("API key saved and Gemini client updated");
    Ok(())
}

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, AppError> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

#[tauri::command]
pub async fn update_config(
    state: State<'_, AppState>,
    new_config: AppConfig,
) -> Result<(), AppError> {
    let old_watch_dirs = {
        let config = state.config.lock().await;
        config.watch_directories.clone()
    };

    config::save_config(&state.data_dir, &new_config)?;

    let watch_dirs_changed = old_watch_dirs != new_config.watch_directories;

    {
        let mut config = state.config.lock().await;
        *config = new_config;
    }

    if watch_dirs_changed {
        restart_watcher_internal(&state).await?;
    }

    info!("configuration updated");
    Ok(())
}

// === Extensions ===

#[tauri::command]
pub fn get_all_supported_extensions() -> Vec<String> {
    crawler::all_supported_extensions()
}

// === File ===

#[tauri::command]
pub async fn open_file(file_path: String) -> Result<(), AppError> {
    tauri_plugin_opener::open_path(&file_path, None::<&str>)
        .map_err(|e| AppError::Internal(format!("failed to open file: {e}")))
}

// === Watcher ===

#[tauri::command]
pub async fn get_watcher_status(state: State<'_, AppState>) -> Result<WatcherStatus, AppError> {
    let watcher = state.watcher.lock().await;
    match watcher.as_ref() {
        Some(w) => Ok(WatcherStatus {
            is_running: true,
            watched_directories: w
                .watched_directories()
                .iter()
                .map(|d| d.display().to_string())
                .collect(),
        }),
        None => Ok(WatcherStatus {
            is_running: false,
            watched_directories: vec![],
        }),
    }
}

#[tauri::command]
pub async fn restart_watcher(state: State<'_, AppState>) -> Result<WatcherStatus, AppError> {
    restart_watcher_internal(&state).await?;
    get_watcher_status(state).await
}

#[tauri::command]
pub async fn stop_watcher(state: State<'_, AppState>) -> Result<(), AppError> {
    let mut watcher = state.watcher.lock().await;
    if watcher.is_some() {
        *watcher = None;
        info!("file watcher stopped");
    }
    Ok(())
}

/// Internal helper to restart the file watcher from the current config.
async fn restart_watcher_internal(state: &AppState) -> Result<(), AppError> {
    let config = state.config.lock().await;
    let directories = config.watch_directories.clone();
    let extensions = config.supported_extensions.clone();
    drop(config);

    let exclude_dirs: Vec<String> = default_exclude_dirs()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let new_watcher = FileWatcher::start(
        directories,
        extensions,
        exclude_dirs,
        state.db.clone(),
        state.tantivy.clone(),
        state.gemini.clone(),
    )?;

    let mut watcher = state.watcher.lock().await;
    *watcher = new_watcher;

    if watcher.is_some() {
        info!("file watcher restarted");
    }

    Ok(())
}

// === Stats ===

#[tauri::command]
pub async fn get_indexed_stats(state: State<'_, AppState>) -> Result<IndexedStats, AppError> {
    let db = state.db.lock().await;
    let total_files = db.get_indexed_count()?;
    let by_extension_vec = db.get_count_by_extension()?;
    let total_size_bytes = db.get_total_size()?;

    let by_extension: HashMap<String, usize> = by_extension_vec.into_iter().collect();

    Ok(IndexedStats {
        total_files,
        by_extension,
        total_size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::IndexingState;

    #[test]
    fn test_search_response_serialization() {
        let response = SearchResponse {
            results: vec![SearchResultItem {
                file_path: "/test/doc.pdf".into(),
                file_name: "doc.pdf".into(),
                summary: "Test summary".into(),
                keywords: r#"["test"]"#.into(),
                final_score: 0.85,
                keyword_score: 0.7,
                vector_score: 0.9,
            }],
            mode_used: "Hybrid".into(),
            query_time_ms: 42,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("doc.pdf"));
        assert!(json.contains("Hybrid"));
    }

    #[test]
    fn test_indexed_stats_serialization() {
        let stats = IndexedStats {
            total_files: 100,
            by_extension: {
                let mut m = HashMap::new();
                m.insert(".pdf".into(), 50);
                m.insert(".docx".into(), 30);
                m
            },
            total_size_bytes: 1024 * 1024,
        };

        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("total_files"));
        assert!(json.contains("by_extension"));
    }

    #[test]
    fn test_frontend_search_mode_deserialization() {
        let hybrid: FrontendSearchMode = serde_json::from_str(r#""Hybrid""#).unwrap();
        assert!(matches!(hybrid, FrontendSearchMode::Hybrid));

        let keyword: FrontendSearchMode = serde_json::from_str(r#""KeywordOnly""#).unwrap();
        assert!(matches!(keyword, FrontendSearchMode::KeywordOnly));

        let vector: FrontendSearchMode = serde_json::from_str(r#""VectorOnly""#).unwrap();
        assert!(matches!(vector, FrontendSearchMode::VectorOnly));
    }

    #[test]
    fn test_get_all_supported_extensions_returns_nonempty_list() {
        let extensions = get_all_supported_extensions();
        assert!(!extensions.is_empty());
        // All extensions must start with a dot
        for ext in &extensions {
            assert!(ext.starts_with('.'), "extension {ext} must start with '.'");
        }
        // Must include common document and image formats
        assert!(extensions.contains(&".pdf".to_string()));
        assert!(extensions.contains(&".txt".to_string()));
        assert!(extensions.contains(&".png".to_string()));
    }

    #[test]
    fn test_frontend_search_filters_deserialization() {
        let json =
            r#"{"file_types":["pdf","docx"],"date_after":1704067200,"directories":["/docs"]}"#;
        let filters: FrontendSearchFilters = serde_json::from_str(json).unwrap();
        assert_eq!(
            filters.file_types.as_ref().unwrap(),
            &vec!["pdf".to_string(), "docx".to_string()]
        );
        assert_eq!(filters.date_after, Some(1704067200));
        assert!(filters.date_before.is_none());

        let search_filters = filters.into_search_filters();
        assert_eq!(search_filters.file_extensions.len(), 2);
        assert_eq!(search_filters.directories, vec!["/docs".to_string()]);
    }

    #[test]
    fn test_frontend_search_filters_empty_deserialization() {
        let json = r#"{}"#;
        let filters: FrontendSearchFilters = serde_json::from_str(json).unwrap();
        let search_filters = filters.into_search_filters();
        assert!(!search_filters.has_any_filter());
    }

    #[test]
    fn test_indexing_status_default() {
        let status = IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("Idle"));
    }
}
