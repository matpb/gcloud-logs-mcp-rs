use serde::Deserialize;
use std::env;
use std::fmt;

#[derive(Clone, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub project_id: String,
    #[serde(default)]
    pub credentials_file: Option<String>,
}

impl fmt::Debug for ProjectConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectConfig")
            .field("name", &self.name)
            .field("project_id", &self.project_id)
            .field(
                "credentials_file",
                &self.credentials_file.as_ref().map(|_| "[SET]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub projects: Vec<ProjectConfig>,
    pub default_limit: u32,
    pub max_limit: u32,
}

impl Config {
    pub fn from_env() -> Self {
        let projects_json = env::var("GCP_PROJECTS")
            .expect("GCP_PROJECTS env var is required (JSON array of project configs)");

        let projects: Vec<ProjectConfig> = serde_json::from_str(&projects_json)
            .expect("GCP_PROJECTS must be valid JSON array");

        if projects.is_empty() {
            panic!("GCP_PROJECTS must contain at least one project config");
        }

        Self {
            host: env::var("MCP_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("MCP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8432),
            projects,
            default_limit: env::var("DEFAULT_LOG_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            max_limit: env::var("MAX_LOG_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
        }
    }

    pub fn project_names(&self) -> Vec<&str> {
        self.projects.iter().map(|p| p.name.as_str()).collect()
    }
}
