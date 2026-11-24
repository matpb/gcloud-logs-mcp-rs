use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Implementation, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::config::Config;
use crate::logging::{LoggingClient, QueryParams};

#[derive(Clone)]
pub struct GcloudLogsMcp {
    client: Arc<LoggingClient>,
    config: Config,
    tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
}

impl GcloudLogsMcp {
    pub fn new(client: Arc<LoggingClient>, config: Config) -> Self {
        let tool_router = Self::tool_router();
        Self {
            client,
            config,
            tool_router,
        }
    }
}

// --- Parameter types ---

#[derive(Debug, Deserialize, JsonSchema)]
struct ProjectParams {
    /// Project name (e.g. "my-dev", "my-prod")
    project: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryLogsParams {
    /// Project name (e.g. "my-dev", "my-prod")
    project: String,
    /// Cloud Logging filter expression (e.g. 'resource.type="cloud_run_revision" AND textPayload:"error"')
    filter: Option<String>,
    /// Filter by resource type (e.g. "gce_instance", "cloud_run_revision", "cloud_function")
    resource_type: Option<String>,
    /// Minimum severity: DEFAULT, DEBUG, INFO, NOTICE, WARNING, ERROR, CRITICAL, ALERT, EMERGENCY
    severity: Option<String>,
    /// Time range: relative ("1h", "30m", "7d", "2w") or ISO timestamp ("2024-01-15T00:00:00Z") or range ("start/end")
    time_range: Option<String>,
    /// Max entries to return (default: 100, max: 1000)
    limit: Option<u32>,
    /// Order: "timestamp asc" or "timestamp desc" (default: "timestamp desc")
    order_by: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetLogEntryParams {
    /// Project name (e.g. "my-dev", "my-prod")
    project: String,
    /// The insertId of the log entry to retrieve
    insert_id: String,
}

/// Build the `insertId` filter, escaping backslashes before quotes so the value stays one string literal.
fn insert_id_filter(insert_id: &str) -> String {
    let escaped = insert_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!("insertId=\"{}\"", escaped)
}

// --- Tool implementations ---

#[tool_router]
impl GcloudLogsMcp {
    #[tool(
        name = "list_projects",
        description = "List all configured GCP projects and their details"
    )]
    async fn list_projects(&self) -> Result<String, rmcp::ErrorData> {
        let projects: Vec<serde_json::Value> = self
            .config
            .projects
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "project_id": p.project_id,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "projects": projects,
            "count": projects.len(),
            "message": format!("Available projects: {}", self.config.project_names().join(", "))
        })
        .to_string())
    }

    #[tool(
        name = "list_logs",
        description = "List all available log names in a GCP project"
    )]
    async fn list_logs(
        &self,
        Parameters(p): Parameters<ProjectParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let logs = self.client.list_logs(&p.project).await.map_err(|e| {
            tracing::error!(project = %p.project, error = %e, "list_logs failed");
            rmcp::ErrorData::internal_error(e, None)
        })?;

        Ok(serde_json::json!({
            "logs": logs,
            "count": logs.len(),
            "project": p.project,
            "message": format!("Found {} log(s) in project '{}'", logs.len(), p.project)
        })
        .to_string())
    }

    #[tool(
        name = "query_logs",
        description = "Query log entries from Google Cloud Logging. Supports Cloud Logging filter syntax, resource type filtering, severity filtering, and flexible time ranges."
    )]
    async fn query_logs(
        &self,
        Parameters(p): Parameters<QueryLogsParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let limit = p
            .limit
            .unwrap_or(self.config.default_limit)
            .min(self.config.max_limit);

        let params = QueryParams {
            filter: p.filter,
            resource_type: p.resource_type,
            severity: p.severity,
            time_range: p.time_range,
            limit,
            order_by: p.order_by,
        };

        let result = self
            .client
            .query_entries(&p.project, &params)
            .await
            .map_err(|e| {
                tracing::error!(project = %p.project, error = %e, "query_logs failed");
                rmcp::ErrorData::internal_error(e, None)
            })?;

        Ok(serde_json::json!({
            "success": true,
            "project": p.project,
            "entries": result.entries,
            "count": result.count,
            "has_more": result.next_page_token.is_some(),
            "executionTime": format!("{}ms", result.elapsed_ms),
            "message": format!("Query returned {} log entries in {}ms", result.count, result.elapsed_ms)
        })
        .to_string())
    }

    #[tool(
        name = "get_log_entry",
        description = "Get a specific log entry by its insertId"
    )]
    async fn get_log_entry(
        &self,
        Parameters(p): Parameters<GetLogEntryParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let params = QueryParams {
            filter: Some(insert_id_filter(&p.insert_id)),
            resource_type: None,
            severity: None,
            time_range: None,
            limit: 1,
            order_by: None,
        };

        let result = self
            .client
            .query_entries(&p.project, &params)
            .await
            .map_err(|e| {
                tracing::error!(project = %p.project, error = %e, "get_log_entry failed");
                rmcp::ErrorData::internal_error(e, None)
            })?;

        if let Some(entry) = result.entries.into_iter().next() {
            Ok(serde_json::json!({
                "success": true,
                "project": p.project,
                "entry": entry,
                "message": format!("Found log entry with insertId '{}'", p.insert_id)
            })
            .to_string())
        } else {
            Ok(serde_json::json!({
                "success": false,
                "project": p.project,
                "message": format!("No log entry found with insertId '{}'. Note: entries may have been deleted or the ID may be incorrect.", p.insert_id)
            })
            .to_string())
        }
    }

    #[tool(
        name = "list_resource_types",
        description = "List resource types that have recent log entries in a GCP project (e.g. gce_instance, cloud_run_revision, cloud_function)"
    )]
    async fn list_resource_types(
        &self,
        Parameters(p): Parameters<ProjectParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let types = self
            .client
            .list_resource_types(&p.project)
            .await
            .map_err(|e| {
                tracing::error!(project = %p.project, error = %e, "list_resource_types failed");
                rmcp::ErrorData::internal_error(e, None)
            })?;

        Ok(serde_json::json!({
            "resource_types": types,
            "count": types.len(),
            "project": p.project,
            "message": format!("Found {} resource type(s) with recent logs in '{}'", types.len(), p.project)
        })
        .to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GcloudLogsMcp {
    fn get_info(&self) -> ServerConfig {
        let project_names = self.config.project_names();
        let mut info = ServerConfig::default();
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.instructions = Some(format!(
            "GCloud Logs MCP Server — read-only access to Google Cloud Logging for {} project(s): {}. \
             Use list_projects to discover available projects. \
             All tools require a 'project' parameter. \
             Use query_logs for log queries (supports Cloud Logging filter syntax), \
             list_logs to see available log names, \
             list_resource_types to discover resource types, \
             get_log_entry to fetch a specific entry by insertId. \
             Resources are available at gcp-logs://<project-name> for each configured project.",
            project_names.len(),
            project_names.join(", ")
        ));
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resources: Vec<Resource> = self
            .config
            .projects
            .iter()
            .map(|p| {
                Resource::new(format!("gcp-logs://{}", p.name), p.name.clone())
                    .with_description(format!(
                        "Google Cloud Logging for project '{}' ({})",
                        p.name, p.project_id
                    ))
                    .with_mime_type("application/json")
            })
            .collect();

        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let project_name = request
            .uri
            .strip_prefix("gcp-logs://")
            .ok_or_else(|| ErrorData::invalid_params("URI must start with gcp-logs://", None))?;

        let project_config = self
            .config
            .projects
            .iter()
            .find(|p| p.name == project_name)
            .ok_or_else(|| {
                let available: Vec<&str> = self
                    .config
                    .projects
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                ErrorData::invalid_params(
                    format!("Unknown project '{project_name}'. Available: {available:?}"),
                    None,
                )
            })?;

        let info = serde_json::json!({
            "name": project_config.name,
            "project_id": project_config.project_id,
            "usage": format!(
                "Use project name '{}' as the 'project' parameter in query_logs, list_logs, list_resource_types, and get_log_entry tools.",
                project_config.name
            )
        });

        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            serde_json::to_string_pretty(&info).unwrap_or_default(),
            request.uri,
        )])
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthManager;
    use crate::config::ProjectConfig;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const INSTRUCTIONS: &str = "GCloud Logs MCP Server — read-only access to Google Cloud Logging for 2 project(s): my-dev, my-prod. Use list_projects to discover available projects. All tools require a 'project' parameter. Use query_logs for log queries (supports Cloud Logging filter syntax), list_logs to see available log names, list_resource_types to discover resource types, get_log_entry to fetch a specific entry by insertId. Resources are available at gcp-logs://<project-name> for each configured project.";

    const TOOLS_LIST: &str = r#"{
  "tools": [
    {
      "name": "get_log_entry",
      "description": "Get a specific log entry by its insertId",
      "inputSchema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "properties": {
          "insert_id": {
            "description": "The insertId of the log entry to retrieve",
            "type": "string"
          },
          "project": {
            "description": "Project name (e.g. \"my-dev\", \"my-prod\")",
            "type": "string"
          }
        },
        "required": [
          "project",
          "insert_id"
        ],
        "type": "object"
      }
    },
    {
      "name": "list_logs",
      "description": "List all available log names in a GCP project",
      "inputSchema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "properties": {
          "project": {
            "description": "Project name (e.g. \"my-dev\", \"my-prod\")",
            "type": "string"
          }
        },
        "required": [
          "project"
        ],
        "type": "object"
      }
    },
    {
      "name": "list_projects",
      "description": "List all configured GCP projects and their details",
      "inputSchema": {
        "properties": {},
        "type": "object"
      }
    },
    {
      "name": "list_resource_types",
      "description": "List resource types that have recent log entries in a GCP project (e.g. gce_instance, cloud_run_revision, cloud_function)",
      "inputSchema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "properties": {
          "project": {
            "description": "Project name (e.g. \"my-dev\", \"my-prod\")",
            "type": "string"
          }
        },
        "required": [
          "project"
        ],
        "type": "object"
      }
    },
    {
      "name": "query_logs",
      "description": "Query log entries from Google Cloud Logging. Supports Cloud Logging filter syntax, resource type filtering, severity filtering, and flexible time ranges.",
      "inputSchema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "properties": {
          "filter": {
            "description": "Cloud Logging filter expression (e.g. 'resource.type=\"cloud_run_revision\" AND textPayload:\"error\"')",
            "type": [
              "string",
              "null"
            ]
          },
          "limit": {
            "description": "Max entries to return (default: 100, max: 1000)",
            "format": "uint32",
            "minimum": 0,
            "type": [
              "integer",
              "null"
            ]
          },
          "order_by": {
            "description": "Order: \"timestamp asc\" or \"timestamp desc\" (default: \"timestamp desc\")",
            "type": [
              "string",
              "null"
            ]
          },
          "project": {
            "description": "Project name (e.g. \"my-dev\", \"my-prod\")",
            "type": "string"
          },
          "resource_type": {
            "description": "Filter by resource type (e.g. \"gce_instance\", \"cloud_run_revision\", \"cloud_function\")",
            "type": [
              "string",
              "null"
            ]
          },
          "severity": {
            "description": "Minimum severity: DEFAULT, DEBUG, INFO, NOTICE, WARNING, ERROR, CRITICAL, ALERT, EMERGENCY",
            "type": [
              "string",
              "null"
            ]
          },
          "time_range": {
            "description": "Time range: relative (\"1h\", \"30m\", \"7d\", \"2w\") or ISO timestamp (\"2024-01-15T00:00:00Z\") or range (\"start/end\")",
            "type": [
              "string",
              "null"
            ]
          }
        },
        "required": [
          "project"
        ],
        "type": "object"
      }
    }
  ]
}"#;

    async fn test_server() -> GcloudLogsMcp {
        let config = Config {
            host: "127.0.0.1".into(),
            port: 0,
            projects: vec![
                ProjectConfig {
                    name: "my-dev".into(),
                    project_id: "dev-project-id".into(),
                    credentials_file: None,
                },
                ProjectConfig {
                    name: "my-prod".into(),
                    project_id: "prod-project-id".into(),
                    credentials_file: None,
                },
            ],
            default_limit: 100,
            max_limit: 1000,
            api_key: None,
        };
        let client = Arc::new(LoggingClient::new(AuthManager::new(&[]).await).await);
        GcloudLogsMcp::new(client, config)
    }

    fn handshake() -> Vec<Value> {
        vec![
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        ]
    }

    fn request(id: u64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    /// Sends the messages over an in-memory JSON-RPC transport and returns the responses by id.
    async fn exchange(messages: &[Value]) -> BTreeMap<u64, Value> {
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (server_read, server_write) = tokio::io::split(server_io);
        let (client_read, mut client_write) = tokio::io::split(client_io);
        let mut expected = 0;
        for message in messages {
            if message.get("id").is_some() {
                expected += 1;
            }
            client_write
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap();
        }
        let running = rmcp::serve_server(test_server().await, (server_read, server_write))
            .await
            .unwrap();
        let mut lines = BufReader::new(client_read).lines();
        let mut responses = BTreeMap::new();
        for _ in 0..expected {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("server closed the stream");
            let response: Value = serde_json::from_str(&line).unwrap();
            responses.insert(response["id"].as_u64().unwrap(), response);
        }
        drop(running);
        responses
    }

    #[tokio::test]
    async fn initialize_advertises_only_tools_and_resources() {
        let responses = exchange(&handshake()).await;
        let result = &responses[&1]["result"];
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert_eq!(result["serverInfo"]["name"], "gcloud-logs-mcp");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            result["capabilities"],
            json!({"tools": {}, "resources": {}})
        );
        assert_eq!(result["instructions"], INSTRUCTIONS);
    }

    #[tokio::test]
    async fn tools_list_snapshot() {
        let mut messages = handshake();
        messages.push(request(2, "tools/list", json!({})));
        let responses = exchange(&messages).await;
        let expected: Value = serde_json::from_str(TOOLS_LIST).unwrap();
        assert_eq!(responses[&2]["result"], expected);
    }

    #[tokio::test]
    async fn list_projects_call_snapshot() {
        let mut messages = handshake();
        messages.push(request(
            3,
            "tools/call",
            json!({"name": "list_projects", "arguments": {}}),
        ));
        let responses = exchange(&messages).await;
        assert_eq!(
            responses[&3]["result"],
            json!({
                "content": [{
                    "type": "text",
                    "text": "{\"count\":2,\"message\":\"Available projects: my-dev, my-prod\",\"projects\":[{\"name\":\"my-dev\",\"project_id\":\"dev-project-id\"},{\"name\":\"my-prod\",\"project_id\":\"prod-project-id\"}]}"
                }],
                "isError": false
            })
        );
    }

    #[tokio::test]
    async fn resources_list_snapshot() {
        let mut messages = handshake();
        messages.push(request(4, "resources/list", json!({})));
        let responses = exchange(&messages).await;
        assert_eq!(
            responses[&4]["result"],
            json!({
                "resources": [
                    {
                        "uri": "gcp-logs://my-dev",
                        "name": "my-dev",
                        "description": "Google Cloud Logging for project 'my-dev' (dev-project-id)",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "gcp-logs://my-prod",
                        "name": "my-prod",
                        "description": "Google Cloud Logging for project 'my-prod' (prod-project-id)",
                        "mimeType": "application/json"
                    }
                ]
            })
        );
    }

    #[tokio::test]
    async fn resources_read_returns_project_info() {
        let mut messages = handshake();
        messages.push(request(
            5,
            "resources/read",
            json!({"uri": "gcp-logs://my-dev"}),
        ));
        let responses = exchange(&messages).await;
        assert_eq!(
            responses[&5]["result"],
            json!({
                "contents": [{
                    "uri": "gcp-logs://my-dev",
                    "mimeType": "text/plain",
                    "text": "{\n  \"name\": \"my-dev\",\n  \"project_id\": \"dev-project-id\",\n  \"usage\": \"Use project name 'my-dev' as the 'project' parameter in query_logs, list_logs, list_resource_types, and get_log_entry tools.\"\n}"
                }]
            })
        );
    }

    #[tokio::test]
    async fn resources_read_rejects_unknown_project_and_scheme() {
        let mut messages = handshake();
        messages.push(request(
            6,
            "resources/read",
            json!({"uri": "gcp-logs://missing"}),
        ));
        messages.push(request(
            7,
            "resources/read",
            json!({"uri": "https://example.invalid/x"}),
        ));
        let responses = exchange(&messages).await;
        assert_eq!(
            responses[&6]["error"],
            json!({
                "code": -32602,
                "message": "Unknown project 'missing'. Available: [\"my-dev\", \"my-prod\"]"
            })
        );
        assert_eq!(
            responses[&7]["error"],
            json!({"code": -32602, "message": "URI must start with gcp-logs://"})
        );
    }

    #[test]
    fn insert_id_filter_escapes_quotes_and_backslashes() {
        assert_eq!(insert_id_filter(r#"ab"c\d"#), r#"insertId="ab\"c\\d""#);
        assert_eq!(insert_id_filter(r#"\""#), r#"insertId="\\\"""#);
        assert_eq!(insert_id_filter("plain-id_1"), r#"insertId="plain-id_1""#);
        assert_eq!(insert_id_filter(""), r#"insertId="""#);
    }
}
