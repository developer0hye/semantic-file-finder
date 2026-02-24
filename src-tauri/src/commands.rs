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
use crate::tantivy_index::TantivyIndex;

/// Shared application state managed by Tauri.
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub tantivy: Arc<Mutex<TantivyIndex>>,
    pub gemini: Arc<Mutex<Option<GeminiClient>>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub data_dir: PathBuf,
    pub indexing_status: Arc<Mutex<IndexingStatus>>,
    pub pause_flag: Arc<AtomicBool>,
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

// === Search ===

#[tauri::command]
pub async fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
    mode: Option<FrontendSearchMode>,
    alpha: Option<f32>,
) -> Result<SearchResponse, AppError> {
    let start = std::time::Instant::now();
    let limit = limit.unwrap_or(20);
    let alpha = alpha.unwrap_or(0.4);

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
    let document_embeddings = db_guard.get_all_embeddings()?;

    let mode_used = if query_embedding.is_some() && search_mode != SearchMode::KeywordOnly {
        search_mode
    } else {
        SearchMode::KeywordOnly
    };

    let results = search::hybrid_search(
        &tantivy_guard,
        &query,
        query_embedding.as_deref(),
        &document_embeddings,
        mode_used,
        alpha,
        limit,
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
    config::save_config(&state.data_dir, &new_config)?;
    let mut config = state.config.lock().await;
    *config = new_config;
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
