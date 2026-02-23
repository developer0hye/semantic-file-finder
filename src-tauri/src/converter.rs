use std::path::Path;

use anytomd::ConversionOptions;

use crate::error::AppError;

/// Wrapper result from document conversion.
pub struct ConversionOutput {
    pub markdown: String,
    pub plain_text: String,
    pub images: Vec<ImageData>,
    pub title: Option<String>,
    pub warnings: Vec<String>,
}

/// Image extracted from a document during conversion.
pub struct ImageData {
    pub filename: String,
    pub data: Vec<u8>,
}

fn map_result(result: anytomd::ConversionResult) -> ConversionOutput {
    ConversionOutput {
        markdown: result.markdown,
        plain_text: result.plain_text,
        images: result
            .images
            .into_iter()
            .map(|(filename, data)| ImageData { filename, data })
            .collect(),
        title: result.title,
        warnings: result
            .warnings
            .into_iter()
            .map(|w| format!("{:?}", w))
            .collect(),
    }
}

/// Convert a file at the given path to Markdown using anytomd.
pub fn convert_file(path: &Path) -> Result<ConversionOutput, AppError> {
    let result = anytomd::convert_file(path, &ConversionOptions::default()).map_err(|e| {
        AppError::ConversionFailed {
            path: path.display().to_string(),
            detail: e.to_string(),
        }
    })?;
    Ok(map_result(result))
}

/// Convert raw bytes with a known format extension to Markdown.
pub fn convert_bytes(data: &[u8], extension: &str) -> Result<ConversionOutput, AppError> {
    let result =
        anytomd::convert_bytes(data, extension, &ConversionOptions::default()).map_err(|e| {
            AppError::ConversionFailed {
                path: format!("<bytes>.{extension}"),
                detail: e.to_string(),
            }
        })?;
    Ok(map_result(result))
}

/// Extensions that anytomd can convert locally (no Gemini upload needed).
const CONVERTIBLE_EXTENSIONS: &[&str] = &[
    "docx", "pptx", "xlsx", "xls", "csv", "json", "txt", "md", "html", "xml",
];

/// Extensions that require Gemini Files API upload for analysis.
const GEMINI_UPLOAD_EXTENSIONS: &[&str] = &["pdf", "jpg", "jpeg", "png", "gif", "webp"];

/// Check if a file extension can be converted locally by anytomd.
pub fn is_convertible(ext: &str) -> bool {
    let ext_lower = ext.trim_start_matches('.').to_lowercase();
    CONVERTIBLE_EXTENSIONS.contains(&ext_lower.as_str())
}

/// Check if a file extension needs Gemini Files API upload.
pub fn needs_gemini_upload(ext: &str) -> bool {
    let ext_lower = ext.trim_start_matches('.').to_lowercase();
    GEMINI_UPLOAD_EXTENSIONS.contains(&ext_lower.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_convertible_docx() {
        assert!(is_convertible("docx"));
        assert!(is_convertible(".docx"));
        assert!(is_convertible("DOCX"));
    }

    #[test]
    fn test_is_convertible_csv() {
        assert!(is_convertible("csv"));
        assert!(is_convertible(".CSV"));
    }

    #[test]
    fn test_is_convertible_pdf_returns_false() {
        assert!(!is_convertible("pdf"));
        assert!(!is_convertible(".pdf"));
    }

    #[test]
    fn test_is_convertible_unknown_returns_false() {
        assert!(!is_convertible("xyz"));
        assert!(!is_convertible(".unknown"));
    }

    #[test]
    fn test_needs_gemini_upload_pdf() {
        assert!(needs_gemini_upload("pdf"));
        assert!(needs_gemini_upload(".pdf"));
        assert!(needs_gemini_upload("PDF"));
    }

    #[test]
    fn test_needs_gemini_upload_images() {
        assert!(needs_gemini_upload("jpg"));
        assert!(needs_gemini_upload(".png"));
        assert!(needs_gemini_upload("webp"));
    }

    #[test]
    fn test_needs_gemini_upload_docx_returns_false() {
        assert!(!needs_gemini_upload("docx"));
        assert!(!needs_gemini_upload("txt"));
    }

    #[test]
    fn test_convert_bytes_plain_text() {
        let text = b"Hello, this is a plain text document.";
        let result = convert_bytes(text, "txt").unwrap();
        assert!(result.markdown.contains("Hello"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_convert_bytes_csv() {
        let csv = b"name,age\nAlice,30\nBob,25";
        let result = convert_bytes(csv, "csv").unwrap();
        assert!(result.markdown.contains("Alice"));
        assert!(result.markdown.contains("Bob"));
    }

    #[test]
    fn test_convert_bytes_json() {
        let json = br#"{"name": "test", "value": 42}"#;
        let result = convert_bytes(json, "json").unwrap();
        assert!(result.markdown.contains("test"));
    }

    #[test]
    fn test_convert_bytes_unsupported_format() {
        let data = b"random binary data";
        let result = convert_bytes(data, "xyz");
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_file_nonexistent() {
        let result = convert_file(Path::new("/nonexistent/file.docx"));
        assert!(result.is_err());
    }
}
