use std::collections::HashMap;
use std::sync::Arc;

use gcp_auth::{CustomServiceAccount, TokenProvider};

use crate::config::ProjectConfig;

const LOGGING_SCOPE: &[&str] = &["https://www.googleapis.com/auth/logging.read"];

pub struct AuthManager {
    providers: HashMap<String, Arc<dyn TokenProvider>>,
    configs: HashMap<String, ProjectConfig>,
}

impl AuthManager {
    pub async fn new(projects: &[ProjectConfig]) -> Self {
        let mut providers = HashMap::new();
        let mut configs = HashMap::new();

        for project in projects {
            let provider: Arc<dyn TokenProvider> = if let Some(ref creds_file) = project.credentials_file {
                tracing::info!(
                    project = %project.name,
                    "Loading credentials from file"
                );
                let sa = CustomServiceAccount::from_file(creds_file)
                    .unwrap_or_else(|e| {
                        panic!(
                            "Failed to load credentials for project '{}' from '{}': {}",
                            project.name, creds_file, e
                        )
                    });
                Arc::new(sa)
            } else {
                tracing::info!(
                    project = %project.name,
                    "Using application default credentials"
                );
                gcp_auth::provider()
                    .await
                    .unwrap_or_else(|e| {
                        panic!(
                            "Failed to find default credentials for project '{}': {}",
                            project.name, e
                        )
                    })
            };

            // Validate credentials by requesting a token
            let _token: std::sync::Arc<gcp_auth::Token> = provider.token(LOGGING_SCOPE).await.unwrap_or_else(|e| {
                panic!(
                    "Failed to acquire initial token for project '{}': {}",
                    project.name, e
                )
            });

            providers.insert(project.name.clone(), provider);
            configs.insert(project.name.clone(), project.clone());
        }

        Self { providers, configs }
    }

    pub async fn get_token(&self, project_name: &str) -> Result<String, String> {
        let provider = self
            .providers
            .get(project_name)
            .ok_or_else(|| format!("Unknown project '{}'. Available: {:?}", project_name, self.project_names()))?;

        let token = provider
            .token(LOGGING_SCOPE)
            .await
            .map_err(|e| format!("Failed to get token for project '{}': {}", project_name, e))?;

        Ok(token.as_str().to_string())
    }

    pub fn get_project_id(&self, project_name: &str) -> Result<&str, String> {
        self.configs
            .get(project_name)
            .map(|c| c.project_id.as_str())
            .ok_or_else(|| format!("Unknown project '{}'. Available: {:?}", project_name, self.project_names()))
    }

    pub fn project_names(&self) -> Vec<&str> {
        self.configs.keys().map(|k| k.as_str()).collect()
    }
}
