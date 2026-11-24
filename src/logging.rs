use std::time::Instant;

use chrono::{TimeDelta, Utc};
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
    if (input.contains('T') || input.contains('-') && input.len() > 5)
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input)
    {
        return Ok((dt.to_rfc3339(), None));
    }

    // Parse relative duration: "1h", "30m", "7d", "2w"
    // Split before the last char, which may be multi-byte.
    let unit_start = input.char_indices().last().map_or(0, |(i, _)| i);
    let (num_str, unit) = input.split_at(unit_start);
    let num: i64 = num_str
        .parse()
        .map_err(|_| format!("Invalid time range '{}'. Use formats like '1h', '30m', '7d', or ISO timestamps.", input))?;

    let amount = match unit {
        "s" => TimeDelta::try_seconds(num),
        "m" => TimeDelta::try_minutes(num),
        "h" => TimeDelta::try_hours(num),
        "d" => TimeDelta::try_days(num),
        "w" => TimeDelta::try_weeks(num),
        _ => {
            return Err(format!(
                "Unknown time unit '{}'. Use s, m, h, d, or w.",
                unit
            ))
        }
    };

    if num <= 0 {
        return Err(format!("Time range '{}' must be a positive count.", input));
    }

    // Both the duration and the subtraction from now can be out of range.
    let start = amount
        .and_then(|amount| Utc::now().checked_sub_signed(amount))
        .ok_or_else(|| format!("Time range '{}' is too large.", input))?;
    Ok((start.to_rfc3339(), None))
}

/// Build a Cloud Logging filter string from structured params.
fn build_filter(params: &QueryParams) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref filter) = params.filter
        && !filter.trim().is_empty()
    {
        parts.push(format!("({})", filter));
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
                // Back up to a char boundary so the cut never splits a multi-byte char.
                let mut cut = 2000;
                while !serialized.is_char_boundary(cut) {
                    cut -= 1;
                }
                let truncated = &serialized[..cut];
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
    if let Some(log_name) = result.get("logName").and_then(|v| v.as_str())
        && let Some((_, short)) = log_name.rsplit_once('/')
    {
        result.insert(
            "logName".to_string(),
            serde_json::Value::String(urlencoding_decode(short)),
        );
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
            if hex.len() == 2
                && let Ok(byte) = u8::from_str_radix(&hex, 16)
            {
                result.push(byte as char);
                continue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncates_multibyte_payload_without_panicking() {
        // Serialized, the leading quote puts each "é" at an odd byte, so byte 2000 splits one.
        let payload = "é".repeat(1500);
        let result = format_entry(json!({"textPayload": payload}));
        let truncated = &result["textPayload"];
        assert_eq!(truncated["_truncated"], json!(true));
        assert_eq!(truncated["_original_size"], json!(3002));
        let content = truncated["content"].as_str().expect("content is a string");
        assert!(content.len() <= 2000);
        assert_eq!(content, format!("\"{}", "é".repeat(999)));
    }

    #[test]
    fn short_payload_is_untouched() {
        let result = format_entry(json!({"textPayload": "hi"}));
        assert_eq!(result, json!({"textPayload": "hi"}));
    }

    /// Asserts that `input` means "now minus `expected`", allowing up to 5 seconds of drift.
    fn assert_starts_ago(input: &str, expected: TimeDelta) {
        let (start, end) = parse_time_range(input).expect("valid time range");
        assert_eq!(end, None);
        let start = chrono::DateTime::parse_from_rfc3339(&start).expect("RFC3339 start");
        let ago = Utc::now() - start.with_timezone(&Utc);
        assert!(
            ago >= expected && ago < expected + TimeDelta::seconds(5),
            "{input}: started {ago:?} ago, expected {expected:?}"
        );
    }

    #[test]
    fn parse_time_range_accepts_relative_units() {
        assert_starts_ago("1h", TimeDelta::hours(1));
        assert_starts_ago("30m", TimeDelta::minutes(30));
        assert_starts_ago("7d", TimeDelta::days(7));
        assert_starts_ago("2w", TimeDelta::weeks(2));
        assert_starts_ago("90s", TimeDelta::seconds(90));
    }

    #[test]
    fn parse_time_range_accepts_rfc3339_and_pairs() {
        assert_eq!(
            parse_time_range("2024-01-15T00:00:00Z").unwrap(),
            ("2024-01-15T00:00:00+00:00".to_string(), None)
        );
        assert_eq!(
            parse_time_range("2024-01-15T00:00:00Z/2024-01-16T00:00:00Z").unwrap(),
            (
                "2024-01-15T00:00:00+00:00".to_string(),
                Some("2024-01-16T00:00:00+00:00".to_string())
            )
        );
    }

    #[test]
    fn parse_time_range_rejects_multibyte_input_without_panicking() {
        assert_eq!(
            parse_time_range("é").unwrap_err(),
            "Invalid time range 'é'. Use formats like '1h', '30m', '7d', or ISO timestamps."
        );
        assert_eq!(
            parse_time_range("1é").unwrap_err(),
            "Unknown time unit 'é'. Use s, m, h, d, or w."
        );
        assert_eq!(
            parse_time_range("").unwrap_err(),
            "Invalid time range ''. Use formats like '1h', '30m', '7d', or ISO timestamps."
        );
    }

    #[test]
    fn parse_time_range_rejects_out_of_range_counts_without_panicking() {
        assert_eq!(
            parse_time_range("99999999999999w").unwrap_err(),
            "Time range '99999999999999w' is too large."
        );
        assert_eq!(
            parse_time_range("99999999999999s").unwrap_err(),
            "Time range '99999999999999s' is too large."
        );
        assert_eq!(
            parse_time_range("9999999999999999999h").unwrap_err(),
            "Invalid time range '9999999999999999999h'. Use formats like '1h', '30m', '7d', or ISO timestamps."
        );
    }

    #[test]
    fn parse_time_range_rejects_zero_and_negative_counts() {
        assert_eq!(
            parse_time_range("-5h").unwrap_err(),
            "Time range '-5h' must be a positive count."
        );
        assert_eq!(
            parse_time_range("0m").unwrap_err(),
            "Time range '0m' must be a positive count."
        );
    }

    #[test]
    fn build_filter_wraps_user_filter_in_parentheses() {
        let params = QueryParams {
            filter: Some("a OR b".to_string()),
            resource_type: None,
            severity: Some("error".to_string()),
            time_range: None,
            limit: 10,
            order_by: None,
        };
        assert_eq!(
            build_filter(&params).unwrap(),
            "(a OR b) AND severity>=ERROR"
        );
    }

    #[test]
    fn build_filter_skips_blank_user_filter() {
        let params = QueryParams {
            filter: Some("   ".to_string()),
            resource_type: Some("gce_instance".to_string()),
            severity: None,
            time_range: None,
            limit: 10,
            order_by: None,
        };
        assert_eq!(
            build_filter(&params).unwrap(),
            "resource.type=\"gce_instance\""
        );
    }
}
