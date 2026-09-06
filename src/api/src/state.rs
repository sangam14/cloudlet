use crate::{config::ApiConfig, runtime::SandboxRuntime};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: ApiConfig,
    pub runtime: Arc<SandboxRuntime>,
}

impl AppState {
    pub fn new(config: ApiConfig) -> Self {
        Self {
            config,
            runtime: Arc::new(SandboxRuntime::new()),
        }
    }
}
