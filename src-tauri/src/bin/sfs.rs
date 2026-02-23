use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};
use tokio::sync::Mutex;
use tracing::info;

use semantic_file_search_lib::app_init;
use semantic_file_search_lib::config;
use semantic_file_search_lib::crawler::crawl_directory;
use semantic_file_search_lib::error::AppError;
use semantic_file_search_lib::gemini::GeminiClient;
use semantic_file_search_lib::pipeline::{self, IndexingState, IndexingStatus};
use semantic_file_search_lib::platform::default_exclude_dirs;
use semantic_file_search_lib::search::{self, SearchMode};

/// Semantic File Search CLI for LLM AI tool integration.
#[derive(Parser)]
#[command(name = "sfs", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Search indexed files by natural language query.
    Search {
        /// The search query.
        query: String,
        /// Maximum number of results to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Search mode: hybrid, keyword-only, or vector-only.
        #[arg(long, default_value = "hybrid")]
        mode: String,
        /// Weight for keyword score in hybrid mode (0.0–1.0).
        #[arg(long, default_value_t = 0.4)]
        alpha: f32,
    },
    /// Index files from configured or specified directories.
    Index {
        /// Directories to index. Uses configured watch_directories if omitted.
        #[arg(long)]
        directories: Vec<String>,
        /// Maximum concurrent file processing tasks.
        #[arg(long, default_value_t = 5)]
        concurrency: usize,
    },
    /// Show index status (file counts, sizes).
    Status,
    /// View or modify application configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Display the current configuration.
    Show,
    /// Set a configuration value.
    Set {
        /// Configuration key to set.
        key: String,
        /// New value for the key.
        value: String,
    },
}

fn print_success(data: serde_json::Value) {
    let output = serde_json::json!({"ok": true, "data": data});
    println!(
        "{}",
        serde_json::to_string(&output).expect("JSON serialization failed")
    );
}

fn print_error(code: &str, message: &str) {
    let output = serde_json::json!({
        "ok": false,
        "error": {"code": code, "message": message}
    });
    println!(
        "{}",
        serde_json::to_string(&output).expect("JSON serialization failed")
    );
}

fn init_stderr_logging() {
    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,semantic_file_search_lib=info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
        .init();
}

fn parse_search_mode(mode: &str) -> Result<SearchMode, AppError> {
    match mode {
        "hybrid" => Ok(SearchMode::Hybrid),
        "keyword-only" => Ok(SearchMode::KeywordOnly),
        "vector-only" => Ok(SearchMode::VectorOnly),
        other => Err(AppError::Internal(format!(
            "unknown search mode: {other}. Use hybrid, keyword-only, or vector-only"
        ))),
    }
}

fn app_error_to_code(err: &AppError) -> &'static str {
    match err {
        AppError::FileIo { .. } => "FILE_IO",
        AppError::UnsupportedFormat { .. } => "UNSUPPORTED_FORMAT",
        AppError::InvalidApiKey => "INVALID_API_KEY",
        AppError::Config(_) => "CONFIG",
        AppError::Keychain(_) => "KEYCHAIN",
        AppError::GeminiApi { .. } => "GEMINI_API",
        AppError::GeminiRateLimit { .. } => "GEMINI_RATE_LIMIT",
        AppError::EmbeddingFailed { .. } => "EMBEDDING_FAILED",
        AppError::ConversionFailed { .. } => "CONVERSION_FAILED",
        AppError::Database(_) => "DATABASE",
        AppError::SearchIndex(_) => "SEARCH_INDEX",
        AppError::Internal(_) => "INTERNAL",
    }
}

async fn run_search(query: String, limit: usize, mode: String, alpha: f32) -> Result<(), AppError> {
    let search_mode = parse_search_mode(&mode)?;
    let data_dir = app_init::resolve_data_dir()?;
    let resources = app_init::initialize(&data_dir)?;

    // Build Gemini client from GEMINI_API_KEY env var if available
    let gemini_client = std::env::var("GEMINI_API_KEY").ok().map(|key| {
        GeminiClient::new(
            key,
            resources.config.gemini_model.clone(),
            resources.config.embedding_model.clone(),
            resources.config.embedding_dimensions,
        )
    });

    // Get query embedding if needed
    let query_embedding = if search_mode != SearchMode::KeywordOnly {
        if let Some(client) = gemini_client.as_ref() {
            client.embed_text(&query, "RETRIEVAL_QUERY").await.ok()
        } else {
            None
        }
    } else {
        None
    };

    let document_embeddings = resources.db.get_all_embeddings()?;

    let mode_used = if query_embedding.is_some() && search_mode != SearchMode::KeywordOnly {
        search_mode
    } else {
        SearchMode::KeywordOnly
    };

    let start = std::time::Instant::now();
    let results = search::hybrid_search(
        &resources.tantivy,
        &query,
        query_embedding.as_deref(),
        &document_embeddings,
        mode_used,
        alpha,
        limit,
    )?;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let mode_str = match mode_used {
        SearchMode::Hybrid => "hybrid",
        SearchMode::KeywordOnly => "keyword-only",
        SearchMode::VectorOnly => "vector-only",
    };

    let result_items: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "file_path": r.file_path,
                "file_name": r.file_name,
                "summary": r.summary,
                "keywords": r.keywords,
                "final_score": r.final_score,
                "keyword_score": r.keyword_score,
                "vector_score": r.vector_score,
            })
        })
        .collect();

    print_success(serde_json::json!({
        "results": result_items,
        "mode_used": mode_str,
        "query_time_ms": elapsed_ms,
    }));

    Ok(())
}

async fn run_index(directories: Vec<String>, concurrency: usize) -> Result<(), AppError> {
    let data_dir = app_init::resolve_data_dir()?;
    let resources = app_init::initialize(&data_dir)?;

    let dirs_to_index: Vec<String> = if directories.is_empty() {
        resources
            .config
            .watch_directories
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    } else {
        directories
    };

    if dirs_to_index.is_empty() {
        print_error(
            "CONFIG",
            "no directories to index — pass --directories or configure watch_directories",
        );
        return Ok(());
    }

    let exclude_dirs = default_exclude_dirs();
    let supported_extensions = resources.config.supported_extensions.clone();

    let mut all_entries = Vec::new();
    for dir in &dirs_to_index {
        let entries = crawl_directory(Path::new(dir), &exclude_dirs, &supported_extensions);
        all_entries.extend(entries);
    }

    info!(
        total_files = all_entries.len(),
        directories = ?dirs_to_index,
        "starting indexing"
    );

    // Build Gemini client from GEMINI_API_KEY env var
    let gemini_client: Option<Arc<GeminiClient>> =
        std::env::var("GEMINI_API_KEY").ok().map(|key| {
            Arc::new(GeminiClient::new(
                key,
                resources.config.gemini_model.clone(),
                resources.config.embedding_model.clone(),
                resources.config.embedding_dimensions,
            ))
        });

    if gemini_client.is_some() {
        info!("indexing in full mode (Gemini available)");
    } else {
        info!("indexing in keyword-only mode (no GEMINI_API_KEY)");
    }

    let db = Arc::new(Mutex::new(resources.db));
    let tantivy = Arc::new(Mutex::new(resources.tantivy));
    let status = Arc::new(Mutex::new(IndexingStatus {
        state: IndexingState::Idle,
        total_files: 0,
        indexed_files: 0,
        failed_files: 0,
        current_file: None,
    }));
    let pause_flag = Arc::new(AtomicBool::new(false));

    let total_files = all_entries.len();

    pipeline::run_pipeline(
        gemini_client,
        db,
        tantivy,
        all_entries,
        concurrency,
        status.clone(),
        pause_flag,
    )
    .await?;

    let final_status = status.lock().await;
    print_success(serde_json::json!({
        "total_files": total_files,
        "indexed_files": final_status.indexed_files,
        "failed_files": final_status.failed_files,
    }));

    Ok(())
}

fn run_status() -> Result<(), AppError> {
    let data_dir = app_init::resolve_data_dir()?;
    let resources = app_init::initialize(&data_dir)?;

    let total_files = resources.db.get_indexed_count()?;
    let by_extension_vec = resources.db.get_count_by_extension()?;
    let total_size_bytes = resources.db.get_total_size()?;
    let pending_files = resources.db.get_pending_files()?;

    let by_extension: HashMap<String, usize> = by_extension_vec.into_iter().collect();

    print_success(serde_json::json!({
        "total_files": total_files,
        "by_extension": by_extension,
        "total_size_bytes": total_size_bytes,
        "pending_files": pending_files.len(),
        "data_dir": data_dir.display().to_string(),
    }));

    Ok(())
}

fn run_config_show() -> Result<(), AppError> {
    let data_dir = app_init::resolve_data_dir()?;
    let app_config = config::load_config(&data_dir).unwrap_or_default();
    let json = serde_json::to_value(&app_config)
        .map_err(|e| AppError::Internal(format!("failed to serialize config: {e}")))?;
    print_success(json);
    Ok(())
}

fn run_config_set(key: String, value: String) -> Result<(), AppError> {
    let data_dir = app_init::resolve_data_dir()?;
    let mut app_config = config::load_config(&data_dir).unwrap_or_default();

    match key.as_str() {
        "search_alpha" => {
            let alpha: f32 = value
                .parse()
                .map_err(|_| AppError::Config(format!("invalid float value: {value}")))?;
            app_config.search_alpha = alpha;
        }
        "gemini_model" => {
            app_config.gemini_model = value;
        }
        "embedding_model" => {
            app_config.embedding_model = value;
        }
        "embedding_dimensions" => {
            let dim: u32 = value
                .parse()
                .map_err(|_| AppError::Config(format!("invalid integer value: {value}")))?;
            app_config.embedding_dimensions = dim;
        }
        "supported_extensions" => {
            let exts: Vec<String> = value.split(',').map(|s| s.trim().to_string()).collect();
            app_config.supported_extensions = exts;
        }
        "watch_directories" => {
            let dirs: Vec<std::path::PathBuf> = value.split(',').map(|s| s.trim().into()).collect();
            app_config.watch_directories = dirs;
        }
        other => {
            return Err(AppError::Config(format!("unknown config key: {other}")));
        }
    }

    config::save_config(&data_dir, &app_config)?;
    let json = serde_json::to_value(&app_config)
        .map_err(|e| AppError::Internal(format!("failed to serialize config: {e}")))?;
    print_success(json);
    Ok(())
}

#[tokio::main]
async fn main() {
    init_stderr_logging();
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Search {
            query,
            limit,
            mode,
            alpha,
        } => run_search(query, limit, mode, alpha).await,
        Commands::Index {
            directories,
            concurrency,
        } => run_index(directories, concurrency).await,
        Commands::Status => run_status(),
        Commands::Config { action } => match action {
            ConfigAction::Show => run_config_show(),
            ConfigAction::Set { key, value } => run_config_set(key, value),
        },
    };

    if let Err(e) = result {
        print_error(app_error_to_code(&e), &e.to_string());
        std::process::exit(1);
    }
}
