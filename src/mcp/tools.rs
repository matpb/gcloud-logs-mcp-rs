use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Annotated, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, RawResource, ServerCapabilities, ServerInfo,
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
    /// Project name (e.g. "siku-dev", "siku-prod")
    project: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryLogsParams {
    /// Project name (e.g. "siku-dev", "siku-prod")
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
    /// Project name (e.g. "siku-dev", "siku-prod")
    project: String,
    /// The insertId of the log entry to retrieve
    insert_id: String,
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
            filter: Some(format!("insertId=\"{}\"", p.insert_id)),
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

#[tool_handler]
impl ServerHandler for GcloudLogsMcp {
    fn get_info(&self) -> ServerInfo {
        let project_names = self.config.project_names();
        let mut info = ServerInfo::default();
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

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async {
            let resources: Vec<_> = self
                .config
                .projects
                .iter()
                .map(|p| {
                    Annotated::new(
                        RawResource::new(
                            format!("gcp-logs://{}", p.name),
                            p.name.clone(),
                        )
                        .with_description(format!(
                            "Google Cloud Logging for project '{}' ({})",
                            p.name, p.project_id
                        ))
                        .with_mime_type("application/json"),
                        None,
                    )
                })
                .collect();

            Ok(ListResourcesResult {
                resources,
                ..Default::default()
            })
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        async move {
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
                    let available: Vec<&str> = self.config.projects.iter().map(|p| p.name.as_str()).collect();
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
            )]))
        }
    }
}
