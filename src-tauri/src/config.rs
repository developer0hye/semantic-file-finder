use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Application configuration persisted as JSON at `{app_data_dir}/config.json`.
///
/// API keys are stored in the OS keychain, not in this file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub watch_directories: Vec<PathBuf>,
    pub supported_extensions: Vec<String>,
    pub embedding_model: String,
    pub embedding_dimensions: u32,
    pub gemini_model: String,
    pub search_alpha: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            watch_directories: crate::platform::default_watch_directories(),
            supported_extensions: vec![
                // Documents
                ".pdf".into(),
                ".docx".into(),
                ".pptx".into(),
                ".xlsx".into(),
                ".txt".into(),
                ".md".into(),
                ".csv".into(),
                ".json".into(),
                ".html".into(),
                ".xml".into(),
                // Images
                ".jpg".into(),
                ".png".into(),
                // Source code
                ".c".into(),
                ".h".into(),
                ".cpp".into(),
                ".hpp".into(),
                ".py".into(),
                ".js".into(),
                ".jsx".into(),
                ".ts".into(),
                ".tsx".into(),
                ".rs".into(),
                ".go".into(),
                ".java".into(),
                ".kt".into(),
                ".rb".into(),
                ".swift".into(),
                ".cs".into(),
                ".php".into(),
                ".sh".into(),
                ".lua".into(),
                ".sql".into(),
                ".scala".into(),
                ".dart".into(),
                ".zig".into(),
            ],
            embedding_model: "gemini-embedding-001".into(),
            embedding_dimensions: 1536,
            gemini_model: "gemini-3-flash-preview".into(),
            search_alpha: 0.4,
        }
    }
}

/// Load configuration from `config.json` inside the given directory.
/// Returns the default configuration if the file does not exist.
pub fn load_config(data_dir: &Path) -> Result<AppConfig, crate::error::AppError> {
    let path = data_dir.join("config.json");
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| crate::error::AppError::FileIo {
        path: path.display().to_string(),
        source: e,
    })?;
    let config: AppConfig = serde_json::from_str(&contents)
        .map_err(|e| crate::error::AppError::Config(e.to_string()))?;
    Ok(config)
}

/// Save configuration to `config.json` inside the given directory.
pub fn save_config(data_dir: &Path, config: &AppConfig) -> Result<(), crate::error::AppError> {
    std::fs::create_dir_all(data_dir).map_err(|e| crate::error::AppError::FileIo {
        path: data_dir.display().to_string(),
        source: e,
    })?;
    let path = data_dir.join("config.json");
    let contents = serde_json::to_string_pretty(config)
        .map_err(|e| crate::error::AppError::Config(e.to_string()))?;
    std::fs::write(&path, contents).map_err(|e| crate::error::AppError::FileIo {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_has_expected_values() {
        let config = AppConfig::default();
        assert_eq!(config.embedding_model, "gemini-embedding-001");
        assert_eq!(config.embedding_dimensions, 1536);
        assert_eq!(config.gemini_model, "gemini-3-flash-preview");
        assert!((config.search_alpha - 0.4).abs() < f32::EPSILON);
        assert!(config.supported_extensions.contains(&".pdf".to_string()));
        assert!(config.supported_extensions.contains(&".docx".to_string()));
    }

    #[test]
    fn test_load_config_returns_default_when_file_missing() {
        let tmp = std::env::temp_dir().join("sftest_config_missing");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let config = load_config(&tmp).unwrap();
        assert_eq!(config, AppConfig::default());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_config_save_and_load_roundtrip() {
        let tmp = std::env::temp_dir().join("sftest_config_roundtrip");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let config = AppConfig {
            search_alpha: 0.7,
            watch_directories: vec![PathBuf::from("/test/dir")],
            ..AppConfig::default()
        };

        save_config(&tmp, &config).unwrap();
        let loaded = load_config(&tmp).unwrap();

        assert_eq!(loaded, config);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_config_returns_error_on_invalid_json() {
        let tmp = std::env::temp_dir().join("sftest_config_invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("config.json"), "not valid json").unwrap();

        let result = load_config(&tmp);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_config_serialization_format() {
        let config = AppConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains("watch_directories"));
        assert!(json.contains("embedding_model"));
        assert!(json.contains("search_alpha"));
    }
}
