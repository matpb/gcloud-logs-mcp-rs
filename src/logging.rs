use std::time::Instant;

use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::auth::AuthManager;

const LOGGING_API_BASE: &str = "https://logging.googleapis.com/v2";

pub struct LoggingClient {
    http: reqwest::Client,
    pub auth: AuthManager,
}

pub struct QueryParams {
    pub filter: Option<String>,
    pub resource_type: Option<String>,
    pub severity: Option<String>,
    pub time_range: Option<String>,
    pub limit: u32,
    pub order_by: Option<String>,
}

pub struct QueryResult {
    pub entries: Vec<serde_json::Value>,
    pub count: usize,
    pub next_page_token: Option<String>,
    pub elapsed_ms: u128,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListEntriesResponse {
    #[serde(default)]
    entries: Vec<serde_json::Value>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListLogsResponse {
    #[serde(default)]
    log_names: Vec<String>,
    #[serde(default)]
    next_page_token: Option<String>,
}

impl LoggingClient {
    pub async fn new(auth: AuthManager) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        Self { http, auth }
    }

    pub async fn query_entries(
        &self,
        project_name: &str,
        params: &QueryParams,
    ) -> Result<QueryResult, String> {
        let token = self.auth.get_token(project_name).await?;
        let project_id = self.auth.get_project_id(project_name)?;

        let filter = build_filter(params)?;

        let order_by = params
            .order_by
            .as_deref()
            .unwrap_or("timestamp desc")
            .to_string();

        let mut body = serde_json::json!({
            "resourceNames": [format!("projects/{}", project_id)],
            "pageSize": params.limit,
            "orderBy": order_by,
        });

        if !filter.is_empty() {
            body["filter"] = serde_json::Value::String(filter);
        }

        let start = Instant::now();

        let resp = self
            .http
            .post(format!("{}/entries:list", LOGGING_API_BASE))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let elapsed_ms = start.elapsed().as_millis();

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_api_error(status.as_u16(), &body, project_name));
        }

        let response: ListEntriesResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let entries: Vec<serde_json::Value> = response
            .entries
            .into_iter()
            .map(format_entry)
            .collect();

        let count = entries.len();

        Ok(QueryResult {
            entries,
            count,
            next_page_token: response.next_page_token,
            elapsed_ms,
        })
    }

    pub async fn list_logs(&self, project_name: &str) -> Result<Vec<String>, String> {
        let token = self.auth.get_token(project_name).await?;
        let project_id = self.auth.get_project_id(project_name)?;

        let mut all_logs = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut url = format!("{}/projects/{}/logs", LOGGING_API_BASE, project_id);
            if let Some(ref token) = page_token {
                url = format!("{}?pageToken={}", url, token);
            }

            let resp = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| format!("HTTP request failed: {}", e))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(map_api_error(status.as_u16(), &body, project_name));
            }

            let response: ListLogsResponse = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {}", e))?;

            all_logs.extend(response.log_names);

            match response.next_page_token {
                Some(t) if !t.is_empty() => page_token = Some(t),
                _ => break,
            }
        }

        // Clean up log names: strip the projects/xxx/logs/ prefix and decode percent-encoding
        let cleaned: Vec<String> = all_logs
            .into_iter()
            .map(|name| {
                let short = name
                    .rsplit_once('/')
                    .map(|(_, s)| s.to_string())
                    .unwrap_or(name);
                urlencoding_decode(&short)
            })
            .collect();

        Ok(cleaned)
    }

    pub async fn list_resource_types(&self, project_name: &str) -> Result<Vec<String>, String> {
        // Query recent entries and extract unique resource types
        let params = QueryParams {
            filter: None,
            resource_type: None,
            severity: None,
            time_range: Some("1h".to_string()),
            limit: 1000,
            order_by: Some("timestamp desc".to_string()),
        };

        let result = self.query_entries(project_name, &params).await?;

        let mut types: Vec<String> = result
            .entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("resource")
                    .and_then(|r| r.get("type"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        types.sort();
        Ok(types)
    }
}

/// Parse a time range string into (start_timestamp, optional end_timestamp) as RFC3339 strings.
fn parse_time_range(input: &str) -> Result<(String, Option<String>), String> {
    let input = input.trim();

    // Check for start/end pair: "2024-01-15T00:00:00Z/2024-01-16T00:00:00Z"
    if let Some((start, end)) = input.split_once('/') {
        let start = chrono::DateTime::parse_from_rfc3339(start.trim())
            .map_err(|e| format!("Invalid start timestamp '{}': {}", start, e))?;
        let end = chrono::DateTime::parse_from_rfc3339(end.trim())
            .map_err(|e| format!("Invalid end timestamp '{}': {}", end, e))?;
        return Ok((start.to_rfc3339(), Some(end.to_rfc3339())));
    }

    // Check for ISO timestamp
    if input.contains('T') || input.contains('-') && input.len() > 5 {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input) {
            return Ok((dt.to_rfc3339(), None));
        }
    }

    // Parse relative duration: "1h", "30m", "7d", "2w"
    let (num_str, unit) = input.split_at(input.len().saturating_sub(1));
    let num: i64 = num_str
        .parse()
        .map_err(|_| format!("Invalid time range '{}'. Use formats like '1h', '30m', '7d', or ISO timestamps.", input))?;

    let duration = match unit {
        "s" => Duration::seconds(num),
        "m" => Duration::minutes(num),
        "h" => Duration::hours(num),
        "d" => Duration::days(num),
        "w" => Duration::weeks(num),
        _ => {
            return Err(format!(
                "Unknown time unit '{}'. Use s, m, h, d, or w.",
                unit
            ))
        }
    };

    let start = Utc::now() - duration;
    Ok((start.to_rfc3339(), None))
}

/// Build a Cloud Logging filter string from structured params.
fn build_filter(params: &QueryParams) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref filter) = params.filter {
        if !filter.trim().is_empty() {
            parts.push(filter.clone());
        }
    }

    if let Some(ref resource_type) = params.resource_type {
        parts.push(format!("resource.type=\"{}\"", resource_type));
    }

    if let Some(ref severity) = params.severity {
        let valid = [
            "DEFAULT", "DEBUG", "INFO", "NOTICE", "WARNING", "ERROR", "CRITICAL", "ALERT",
            "EMERGENCY",
        ];
        let upper = severity.to_uppercase();
        if !valid.contains(&upper.as_str()) {
            return Err(format!(
                "Invalid severity '{}'. Valid values: {:?}",
                severity, valid
            ));
        }
        parts.push(format!("severity>={}", upper));
    }

    if let Some(ref time_range) = params.time_range {
        let (start, end) = parse_time_range(time_range)?;
        parts.push(format!("timestamp>=\"{}\"", start));
        if let Some(end) = end {
            parts.push(format!("timestamp<=\"{}\"", end));
        }
    }

    Ok(parts.join(" AND "))
}

/// Format a log entry for concise output, truncating large payloads.
fn format_entry(entry: serde_json::Value) -> serde_json::Value {
    let obj = match entry.as_object() {
        Some(obj) => obj,
        None => return entry,
    };

    let mut result = serde_json::Map::new();

    // Always include these fields
    for key in &[
        "timestamp",
        "severity",
        "logName",
        "insertId",
        "resource",
        "labels",
        "trace",
        "spanId",
    ] {
        if let Some(val) = obj.get(*key) {
            result.insert(key.to_string(), val.clone());
        }
    }

    // Handle payloads with truncation
    for payload_key in &["textPayload", "jsonPayload", "protoPayload"] {
        if let Some(val) = obj.get(*payload_key) {
            let serialized = serde_json::to_string(val).unwrap_or_default();
            if serialized.len() > 2000 {
                let truncated = &serialized[..2000];
                result.insert(
                    payload_key.to_string(),
                    serde_json::json!({
                        "_truncated": true,
                        "_original_size": serialized.len(),
                        "content": truncated,
                    }),
                );
            } else {
                result.insert(payload_key.to_string(), val.clone());
            }
        }
    }

    // Clean up logName: strip projects/xxx/logs/ prefix
    if let Some(log_name) = result.get("logName").and_then(|v| v.as_str()) {
        if let Some((_, short)) = log_name.rsplit_once('/') {
            result.insert(
                "logName".to_string(),
                serde_json::Value::String(urlencoding_decode(short)),
            );
        }
    }

    serde_json::Value::Object(result)
}

/// Decode percent-encoded log names (e.g., "cloudaudit.googleapis.com%2Factivity" → "cloudaudit.googleapis.com/activity")
fn urlencoding_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }
    result
}

/// Map GCP API error responses to user-friendly messages.
fn map_api_error(status: u16, body: &str, project_name: &str) -> String {
    // Try to extract the error message from the JSON response
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.chars().take(500).collect());

    match status {
        401 | 403 => format!(
            "Authentication/authorization failed for project '{}'. Check credentials and ensure the service account has 'roles/logging.viewer'. Detail: {}",
            project_name, detail
        ),
        400 => format!(
            "Invalid request (likely bad filter syntax): {}",
            detail
        ),
        429 => format!(
            "Rate limited by Cloud Logging API. Try again shortly. Detail: {}",
            detail
        ),
        _ => format!(
            "Cloud Logging API error (HTTP {}): {}",
            status, detail
        ),
    }
}
