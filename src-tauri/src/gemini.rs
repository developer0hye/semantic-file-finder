use base64::Engine;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::AppError;

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Result of Gemini document analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentAnalysis {
    pub summary: String,
    pub keywords: Vec<String>,
    pub language: String,
    pub doc_type: String,
}

/// Gemini API client for document analysis and embedding.
pub struct GeminiClient {
    api_key: String,
    model: String,
    embedding_model: String,
    embedding_dimensions: u32,
    http: reqwest::Client,
}

impl GeminiClient {
    pub fn new(
        api_key: String,
        model: String,
        embedding_model: String,
        embedding_dimensions: u32,
    ) -> Self {
        Self {
            api_key,
            model,
            embedding_model,
            embedding_dimensions,
            http: reqwest::Client::new(),
        }
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn embedding_model(&self) -> &str {
        &self.embedding_model
    }

    pub fn embedding_dimensions(&self) -> u32 {
        self.embedding_dimensions
    }

    /// Analyze text content using Gemini generateContent.
    pub async fn analyze_text(&self, text: &str) -> Result<DocumentAnalysis, AppError> {
        let prompt = format!(
            r#"Analyze the following document and respond in JSON.

1. summary: Summarize the document content in 2-3 sentences
2. keywords: Array of 5-10 key terms
3. language: Primary language of the document (ISO 639-1 code)
4. doc_type: Document type (report / contract / presentation / spreadsheet / memo / other)

Response format:
{{"summary": "...", "keywords": ["...", "..."], "language": "en", "doc_type": "report"}}

Document:
{text}"#
        );

        let body = serde_json::json!({
            "contents": [{"parts": [{"text": prompt}]}],
            "generationConfig": {
                "responseMimeType": "application/json"
            }
        });

        let response = self.generate_content(&body).await?;
        Self::parse_analysis_response(&response)
    }

    /// Analyze text content along with images (for DOCX/PPTX with embedded images).
    pub async fn analyze_text_with_images(
        &self,
        text: &str,
        images: &[(String, Vec<u8>)],
    ) -> Result<DocumentAnalysis, AppError> {
        let prompt = r#"Analyze the following document and respond in JSON.

1. summary: Summarize the document content in 2-3 sentences
2. keywords: Array of 5-10 key terms
3. language: Primary language of the document (ISO 639-1 code)
4. doc_type: Document type (report / contract / presentation / spreadsheet / memo / other)

Response format:
{"summary": "...", "keywords": ["...", "..."], "language": "en", "doc_type": "report"}"#;

        let mut parts = Vec::new();
        parts.push(serde_json::json!({"text": format!("{prompt}\n\nDocument:\n{text}")}));

        for (filename, data) in images {
            let mime = mime_from_filename(filename);
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            parts.push(serde_json::json!({
                "inline_data": {
                    "mime_type": mime,
                    "data": b64
                }
            }));
        }

        let body = serde_json::json!({
            "contents": [{"parts": parts}],
            "generationConfig": {
                "responseMimeType": "application/json"
            }
        });

        let response = self.generate_content(&body).await?;
        Self::parse_analysis_response(&response)
    }

    /// Upload a file to the Gemini Files API.
    pub async fn upload_file(
        &self,
        data: &[u8],
        mime_type: &str,
        display_name: &str,
    ) -> Result<String, AppError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",
            self.api_key
        );

        let form = reqwest::multipart::Form::new()
            .text(
                "metadata",
                serde_json::json!({"file": {"display_name": display_name}}).to_string(),
            )
            .part(
                "file",
                reqwest::multipart::Part::bytes(data.to_vec())
                    .mime_str(mime_type)
                    .map_err(|e| AppError::GeminiApi {
                        status_code: 0,
                        message: format!("invalid mime type: {e}"),
                    })?,
            );

        let resp = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| AppError::GeminiApi {
                status_code: 0,
                message: format!("upload request failed: {e}"),
            })?;

        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(AppError::InvalidApiKey);
        }
        if status == 429 {
            return Err(AppError::GeminiRateLimit {
                retry_after_secs: 60,
            });
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| AppError::GeminiApi {
            status_code: status,
            message: format!("failed to parse upload response: {e}"),
        })?;

        let file_uri = body["file"]["uri"]
            .as_str()
            .ok_or_else(|| AppError::GeminiApi {
                status_code: status,
                message: "missing file URI in upload response".into(),
            })?
            .to_string();

        debug!(file_uri = %file_uri, "file uploaded to Gemini");
        Ok(file_uri)
    }

    /// Analyze a previously uploaded file using its Gemini file URI.
    pub async fn analyze_uploaded_file(
        &self,
        file_uri: &str,
        mime_type: &str,
    ) -> Result<DocumentAnalysis, AppError> {
        let prompt = r#"Analyze the following document and respond in JSON.

1. summary: Summarize the document content in 2-3 sentences
2. keywords: Array of 5-10 key terms
3. language: Primary language of the document (ISO 639-1 code)
4. doc_type: Document type (report / contract / presentation / spreadsheet / memo / other)

Response format:
{"summary": "...", "keywords": ["...", "..."], "language": "en", "doc_type": "report"}"#;

        let body = serde_json::json!({
            "contents": [{
                "parts": [
                    {"text": prompt},
                    {"file_data": {"file_uri": file_uri, "mime_type": mime_type}}
                ]
            }],
            "generationConfig": {
                "responseMimeType": "application/json"
            }
        });

        let response = self.generate_content(&body).await?;
        Self::parse_analysis_response(&response)
    }

    /// Generate an embedding vector for the given text.
    pub async fn embed_text(&self, text: &str, task_type: &str) -> Result<Vec<f32>, AppError> {
        let url = format!(
            "{BASE_URL}/models/{}:embedContent?key={}",
            self.embedding_model, self.api_key
        );

        let body = serde_json::json!({
            "model": format!("models/{}", self.embedding_model),
            "content": {"parts": [{"text": text}]},
            "taskType": task_type,
            "outputDimensionality": self.embedding_dimensions
        });

        let resp =
            self.http
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| AppError::GeminiApi {
                    status_code: 0,
                    message: format!("embedding request failed: {e}"),
                })?;

        let status = resp.status().as_u16();
        self.check_error_status(status, &resp).await?;

        let json: serde_json::Value = resp.json().await.map_err(|e| AppError::GeminiApi {
            status_code: status,
            message: format!("failed to parse embedding response: {e}"),
        })?;

        let values =
            json["embedding"]["values"]
                .as_array()
                .ok_or_else(|| AppError::EmbeddingFailed {
                    query: text.chars().take(50).collect(),
                })?;

        let embedding: Vec<f32> = values
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();

        Ok(embedding)
    }

    /// Validate the API key by making a lightweight health check.
    pub async fn validate_api_key(&self) -> Result<bool, AppError> {
        let url = format!("{BASE_URL}/models/{}?key={}", self.model, self.api_key);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::GeminiApi {
                status_code: 0,
                message: format!("API key validation failed: {e}"),
            })?;

        Ok(resp.status().is_success())
    }

    async fn generate_content(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let url = format!(
            "{BASE_URL}/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let resp =
            self.http
                .post(&url)
                .json(body)
                .send()
                .await
                .map_err(|e| AppError::GeminiApi {
                    status_code: 0,
                    message: format!("generateContent request failed: {e}"),
                })?;

        let status = resp.status().as_u16();
        self.check_error_status(status, &resp).await?;

        resp.json().await.map_err(|e| AppError::GeminiApi {
            status_code: status,
            message: format!("failed to parse generateContent response: {e}"),
        })
    }

    async fn check_error_status(
        &self,
        status: u16,
        _resp: &reqwest::Response,
    ) -> Result<(), AppError> {
        if status == 401 || status == 403 {
            return Err(AppError::InvalidApiKey);
        }
        if status == 429 {
            return Err(AppError::GeminiRateLimit {
                retry_after_secs: 60,
            });
        }
        if status >= 500 {
            return Err(AppError::GeminiApi {
                status_code: status,
                message: "Gemini server error".into(),
            });
        }
        Ok(())
    }

    fn parse_analysis_response(response: &serde_json::Value) -> Result<DocumentAnalysis, AppError> {
        let text = response["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or_else(|| AppError::GeminiApi {
                status_code: 200,
                message: "no text in generateContent response".into(),
            })?;

        let analysis: DocumentAnalysis =
            serde_json::from_str(text).map_err(|e| AppError::GeminiApi {
                status_code: 200,
                message: format!("failed to parse analysis JSON: {e}. Raw: {text}"),
            })?;

        Ok(analysis)
    }
}

fn mime_from_filename(filename: &str) -> &str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mime_from_filename_png() {
        assert_eq!(mime_from_filename("image.png"), "image/png");
    }

    #[test]
    fn test_mime_from_filename_jpg() {
        assert_eq!(mime_from_filename("photo.jpg"), "image/jpeg");
        assert_eq!(mime_from_filename("photo.jpeg"), "image/jpeg");
    }

    #[test]
    fn test_mime_from_filename_pdf() {
        assert_eq!(mime_from_filename("document.pdf"), "application/pdf");
    }

    #[test]
    fn test_mime_from_filename_unknown() {
        assert_eq!(mime_from_filename("file.xyz"), "application/octet-stream");
    }

    #[test]
    fn test_parse_analysis_response_valid() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": r#"{"summary":"Test doc","keywords":["a","b"],"language":"en","doc_type":"report"}"#
                    }]
                }
            }]
        });
        let analysis = GeminiClient::parse_analysis_response(&response).unwrap();
        assert_eq!(analysis.summary, "Test doc");
        assert_eq!(analysis.keywords, vec!["a", "b"]);
        assert_eq!(analysis.language, "en");
        assert_eq!(analysis.doc_type, "report");
    }

    #[test]
    fn test_parse_analysis_response_missing_text() {
        let response = serde_json::json!({"candidates": [{"content": {"parts": []}}]});
        assert!(GeminiClient::parse_analysis_response(&response).is_err());
    }

    #[test]
    fn test_parse_analysis_response_invalid_json() {
        let response = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "not json"}]
                }
            }]
        });
        assert!(GeminiClient::parse_analysis_response(&response).is_err());
    }

    #[test]
    fn test_gemini_client_new() {
        let client = GeminiClient::new(
            "test-key".into(),
            "gemini-3-flash-preview".into(),
            "gemini-embedding-001".into(),
            1536,
        );
        assert_eq!(client.api_key, "test-key");
        assert_eq!(client.model, "gemini-3-flash-preview");
        assert_eq!(client.embedding_model, "gemini-embedding-001");
        assert_eq!(client.embedding_dimensions, 1536);
    }
}
