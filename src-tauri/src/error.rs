use serde::Serialize;

/// Unified application error type.
///
/// Variants that depend on crates not yet added (rusqlite, anytomd, etc.)
/// will be introduced in later PRs.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("File I/O error: {path}")]
    FileIo {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Unsupported file format: {extension}")]
    UnsupportedFormat { extension: String },

    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Keychain error: {0}")]
    Keychain(String),

    #[error("Database error")]
    Database(#[from] rusqlite::Error),

    #[error("{0}")]
    Internal(String),
}

/// Serializable error response delivered to the frontend via Tauri IPC.
#[derive(Debug, Serialize, Clone)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl From<&AppError> for ErrorResponse {
    fn from(err: &AppError) -> Self {
        match err {
            AppError::FileIo { path, source } => ErrorResponse {
                code: "FILE_IO".into(),
                message: format!("File I/O error on {path}: {source}"),
                recoverable: true,
            },
            AppError::UnsupportedFormat { extension } => ErrorResponse {
                code: "UNSUPPORTED_FORMAT".into(),
                message: format!("Unsupported file format: {extension}"),
                recoverable: false,
            },
            AppError::InvalidApiKey => ErrorResponse {
                code: "INVALID_API_KEY".into(),
                message: "Invalid API key".into(),
                recoverable: false,
            },
            AppError::Config(msg) => ErrorResponse {
                code: "CONFIG".into(),
                message: format!("Configuration error: {msg}"),
                recoverable: false,
            },
            AppError::Keychain(msg) => ErrorResponse {
                code: "KEYCHAIN".into(),
                message: format!("Keychain error: {msg}"),
                recoverable: true,
            },
            AppError::Database(e) => ErrorResponse {
                code: "DATABASE".into(),
                message: format!("Database error: {e}"),
                recoverable: true,
            },
            AppError::Internal(msg) => ErrorResponse {
                code: "INTERNAL".into(),
                message: msg.clone(),
                recoverable: false,
            },
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let response = ErrorResponse::from(self);
        response.serialize(serializer)
    }
}
