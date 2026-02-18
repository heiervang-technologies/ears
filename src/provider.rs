//! ASR Provider Configuration
//!
//! This module provides configurable ASR/STT provider support, allowing ears
//! to work with different speech-to-text APIs beyond the default OpenAI-compatible
//! endpoint.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Request format type for sending audio to the ASR API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RequestFormat {
    /// Multipart form-data with audio file (OpenAI/Groq style)
    Multipart,
    /// Raw binary audio in request body (Deepgram/Azure style)
    RawBinary,
}

/// Configuration for multipart request format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipartConfig {
    /// Form field name for the audio file
    #[serde(default = "default_file_field")]
    pub file_field: String,
    /// Filename to use in the multipart part
    #[serde(default = "default_filename")]
    pub filename: String,
    /// MIME type for the audio file
    #[serde(default = "default_mime_type")]
    pub mime_type: String,
    /// Additional form text fields (e.g., model, response_format, language)
    #[serde(default)]
    pub extra_fields: HashMap<String, String>,
}

fn default_file_field() -> String {
    "file".to_string()
}

fn default_filename() -> String {
    "recording.wav".to_string()
}

fn default_mime_type() -> String {
    "audio/wav".to_string()
}

impl Default for MultipartConfig {
    fn default() -> Self {
        Self {
            file_field: default_file_field(),
            filename: default_filename(),
            mime_type: default_mime_type(),
            extra_fields: HashMap::new(),
        }
    }
}

/// Configuration for raw binary request format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBinaryConfig {
    /// Content-Type header for the audio body
    #[serde(default = "default_mime_type")]
    pub content_type: String,
    /// Query parameters appended to URL
    #[serde(default)]
    pub query_params: HashMap<String, String>,
}

impl Default for RawBinaryConfig {
    fn default() -> Self {
        Self {
            content_type: default_mime_type(),
            query_params: HashMap::new(),
        }
    }
}

/// Request configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestConfig {
    /// Request format type
    pub format: RequestFormat,
    /// Multipart-specific configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart_config: Option<MultipartConfig>,
    /// Raw binary-specific configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_binary_config: Option<RawBinaryConfig>,
}

/// Response configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseConfig {
    /// JSONPath-like dot notation to extract text from response
    /// Examples: "text", "results.channels[0].alternatives[0].transcript"
    pub text_path: String,
}

/// Health check configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Health check URL (supports variable substitution)
    pub url: String,
    /// HTTP method (GET or HEAD)
    #[serde(default = "default_health_method")]
    pub method: String,
}

fn default_health_method() -> String {
    "GET".to_string()
}

/// ASR Provider Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Human-readable provider name
    pub name: String,
    /// Endpoint URL (supports ${var} substitution)
    pub url: String,
    /// HTTP headers (values support ${var} substitution)
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Request configuration
    pub request: RequestConfig,
    /// Response configuration
    pub response: ResponseConfig,
    /// Optional health check configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<HealthCheckConfig>,
    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    30
}

impl ProviderConfig {
    /// Load provider configuration from a JSON file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read provider config: {}", path.display()))?;
        let config: ProviderConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse provider config: {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate provider configuration
    pub fn validate(&self) -> Result<()> {
        // Validate URL is not empty
        if self.url.is_empty() {
            anyhow::bail!("Provider URL cannot be empty");
        }

        // Validate request format has corresponding config
        match self.request.format {
            RequestFormat::Multipart => {
                if self.request.multipart_config.is_none() {
                    anyhow::bail!("Multipart format requires multipart_config");
                }
            }
            RequestFormat::RawBinary => {
                if self.request.raw_binary_config.is_none() {
                    anyhow::bail!("RawBinary format requires raw_binary_config");
                }
            }
        }

        // Validate text_path is not empty
        if self.response.text_path.is_empty() {
            anyhow::bail!("Response text_path cannot be empty");
        }

        // Validate timeout is reasonable (1-300 seconds, capped at 120 for safety)
        if self.timeout_secs == 0 || self.timeout_secs > 120 {
            anyhow::bail!("timeout_secs must be between 1 and 120 seconds");
        }

        Ok(())
    }

    /// Substitute variables in a string
    ///
    /// Supported variables:
    /// - ${SERVER} - Base server URL
    /// - ${API_KEY} - API authentication key (masked in logs)
    /// - ${LANGUAGE} - Language code
    /// - ${MODEL} - Model identifier
    pub fn substitute_vars(
        &self,
        template: &str,
        vars: &HashMap<String, String>,
    ) -> Result<String> {
        let mut result = template.to_string();

        // Find all variables in the template
        let var_pattern = regex::Regex::new(r"\$\{([A-Z_]+)\}").unwrap();
        for cap in var_pattern.captures_iter(template) {
            let var_name = &cap[1];
            let placeholder = &cap[0];

            // Get variable value
            let value = vars
                .get(var_name)
                .ok_or_else(|| anyhow::anyhow!("Undefined variable: {}", var_name))?;

            result = result.replace(placeholder, value);
        }

        Ok(result)
    }

    /// Build variable map from runtime values
    pub fn build_vars(
        server: Option<&str>,
        api_key: Option<&str>,
        language: Option<&str>,
        model: Option<&str>,
    ) -> HashMap<String, String> {
        let mut vars = HashMap::new();

        if let Some(s) = server {
            vars.insert("SERVER".to_string(), s.to_string());
        }
        if let Some(k) = api_key {
            vars.insert("API_KEY".to_string(), k.to_string());
        }
        if let Some(l) = language {
            vars.insert("LANGUAGE".to_string(), l.to_string());
        }
        if let Some(m) = model {
            vars.insert("MODEL".to_string(), m.to_string());
        }

        vars
    }

    /// Validate URL security (HTTPS for remote, HTTP only for localhost)
    pub fn validate_url_security(url: &str) -> Result<()> {
        let parsed = url::Url::parse(url)
            .with_context(|| format!("Invalid URL: {}", url))?;

        // Allow HTTP only for localhost/127.0.0.1/::1
        if parsed.scheme() == "http" {
            match parsed.host_str() {
                Some("localhost") | Some("127.0.0.1") | Some("[::1]") => {
                    // Allow HTTP for local development
                    Ok(())
                }
                _ => {
                    anyhow::bail!(
                        "HTTPS required for remote URLs (got HTTP for {})",
                        parsed.host_str().unwrap_or("unknown")
                    );
                }
            }
        } else if parsed.scheme() == "https" {
            Ok(())
        } else {
            anyhow::bail!("URL scheme must be http or https, got: {}", parsed.scheme());
        }
    }

    /// Extract text from JSON response using the configured path
    pub fn extract_text_from_response(&self, json: &serde_json::Value) -> Result<String> {
        extract_json_path(json, &self.response.text_path)
    }

    /// Create default OpenAI-compatible provider config (current behavior)
    pub fn default_openai_compatible() -> Self {
        let mut extra_fields = HashMap::new();
        extra_fields.insert("response_format".to_string(), "json".to_string());

        Self {
            name: "OpenAI-compatible (default)".to_string(),
            url: "${SERVER}/v1/audio/transcriptions".to_string(),
            headers: HashMap::new(),
            request: RequestConfig {
                format: RequestFormat::Multipart,
                multipart_config: Some(MultipartConfig {
                    file_field: "file".to_string(),
                    filename: "recording.wav".to_string(),
                    mime_type: "audio/wav".to_string(),
                    extra_fields,
                }),
                raw_binary_config: None,
            },
            response: ResponseConfig {
                text_path: "text".to_string(),
            },
            health_check: Some(HealthCheckConfig {
                url: "${SERVER}/health".to_string(),
                method: "GET".to_string(),
            }),
            timeout_secs: 30,
        }
    }
}

/// Extract value from JSON using dot notation path
///
/// Supports:
/// - Simple field access: "text"
/// - Nested fields: "results.channels"
/// - Array indexing: "results[0].alternatives[0].transcript"
fn extract_json_path(json: &serde_json::Value, path: &str) -> Result<String> {
    let mut current = json;
    let parts = parse_json_path(path);

    for part in parts {
        match part {
            PathSegment::Field(name) => {
                current = current
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("Field '{}' not found in response", name))?;
            }
            PathSegment::Index(idx) => {
                current = current
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("Array index {} not found in response", idx))?;
            }
        }
    }

    // Extract final string value
    match current {
        serde_json::Value::String(s) => Ok(s.clone()),
        _ => anyhow::bail!("Path '{}' did not resolve to a string value", path),
    }
}

#[derive(Debug)]
enum PathSegment {
    Field(String),
    Index(usize),
}

/// Parse JSON path into segments
///
/// Examples:
/// - "text" -> [Field("text")]
/// - "results.channels[0].alternatives[0].transcript" ->
///   [Field("results"), Field("channels"), Index(0), Field("alternatives"), Index(0), Field("transcript")]
fn parse_json_path(path: &str) -> Vec<PathSegment> {
    let mut segments = Vec::new();
    let mut current = String::new();

    for ch in path.chars() {
        match ch {
            '.' => {
                if !current.is_empty() {
                    segments.push(PathSegment::Field(current.clone()));
                    current.clear();
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(PathSegment::Field(current.clone()));
                    current.clear();
                }
            }
            ']' => {
                if !current.is_empty() {
                    if let Ok(idx) = current.parse::<usize>() {
                        segments.push(PathSegment::Index(idx));
                    }
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        segments.push(PathSegment::Field(current));
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_path_simple() {
        let segments = parse_json_path("text");
        assert_eq!(segments.len(), 1);
        matches!(segments[0], PathSegment::Field(ref s) if s == "text");
    }

    #[test]
    fn test_parse_json_path_nested() {
        let segments = parse_json_path("results.channels");
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn test_parse_json_path_with_array() {
        let segments = parse_json_path("results[0].alternatives[0].transcript");
        assert_eq!(segments.len(), 5);
    }

    #[test]
    fn test_extract_text_simple() {
        let json = serde_json::json!({"text": "hello world"});
        let result = extract_json_path(&json, "text").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_extract_text_nested() {
        let json = serde_json::json!({
            "results": {
                "channels": [{
                    "alternatives": [{
                        "transcript": "nested text"
                    }]
                }]
            }
        });
        let result = extract_json_path(&json, "results.channels[0].alternatives[0].transcript").unwrap();
        assert_eq!(result, "nested text");
    }

    #[test]
    fn test_substitute_vars() {
        let config = ProviderConfig::default_openai_compatible();
        let mut vars = HashMap::new();
        vars.insert("SERVER".to_string(), "http://localhost:8178".to_string());
        vars.insert("LANGUAGE".to_string(), "en".to_string());

        let result = config.substitute_vars("${SERVER}/api?lang=${LANGUAGE}", &vars).unwrap();
        assert_eq!(result, "http://localhost:8178/api?lang=en");
    }

    #[test]
    fn test_validate_url_security_https() {
        assert!(ProviderConfig::validate_url_security("https://api.example.com").is_ok());
    }

    #[test]
    fn test_validate_url_security_localhost_http() {
        assert!(ProviderConfig::validate_url_security("http://localhost:8178").is_ok());
        assert!(ProviderConfig::validate_url_security("http://127.0.0.1:8178").is_ok());
    }

    #[test]
    fn test_validate_url_security_remote_http_rejected() {
        assert!(ProviderConfig::validate_url_security("http://api.example.com").is_err());
    }

    #[test]
    fn test_default_openai_compatible() {
        let config = ProviderConfig::default_openai_compatible();
        assert_eq!(config.response.text_path, "text");
        assert_eq!(config.request.format, RequestFormat::Multipart);
        assert!(config.validate().is_ok());
    }
}
