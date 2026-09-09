use crate::{config::ApiConfig, models::ModelRuntime, runtime::SandboxRuntime};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: ApiConfig,
    pub runtime: Arc<SandboxRuntime>,
    pub models: Arc<ModelRuntime>,
}

impl AppState {
    pub fn new(config: ApiConfig) -> Self {
        Self {
            config,
            runtime: Arc::new(SandboxRuntime::new()),
            models: Arc::new(ModelRuntime::from_env()),
        }
    }
}
