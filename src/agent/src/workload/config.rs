use crate::{
    agent::{execute_request, ExecuteRequest},
    agents::Language,
    AgentError, AgentResult,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

const MAX_WORKLOAD_NAME_BYTES: usize = 64;
const MAX_SOURCE_BYTES: usize = 240 * 1024;
const MAX_CONFIG_BYTES: usize = 4 * 1024;
const MAX_ENV_VALUE_BYTES: usize = 2 * 1024;
const ALLOWED_WORKLOAD_ENVIRONMENT: [&str; 3] =
    ["CLOUDLET_LLM_BASE_URL", "OPENAI_BASE_URL", "OPENAI_API_KEY"];

/// Generic agent configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    /// Name of the worklod, used to identify the workload.
    pub workload_name: String,
    /// Language of the workload.
    pub language: Language,
    /// Action to perform.
    pub action: Action,
    /// Code
    pub code: String,
    /// Rest of the configuration as a string.
    pub config_string: String,
    /// Environment variables injected by the VMM for the workload process.
    #[serde(default)]
    pub environment: HashMap<String, String>,
}

impl Config {
    pub fn from_file(file_path: &PathBuf) -> AgentResult<Self> {
        let config = std::fs::read_to_string(file_path).map_err(AgentError::OpenConfigFileError)?;
        let mut config: Config = toml::from_str(&config).map_err(AgentError::ParseConfigError)?;

        let config_string =
            std::fs::read_to_string(file_path).map_err(AgentError::OpenConfigFileError)?;

        config.config_string = config_string;

        config.validate()?;
        Ok(config)
    }

    pub fn new_from_execute_request(execute_request: ExecuteRequest) -> Result<Self, AgentError> {
        let config = Self {
            workload_name: execute_request.workload_name.clone(),
            language: Language::try_from(execute_request.language.clone().as_str())?,
            action: execute_request.action().into(),
            config_string: execute_request.config_str,
            code: execute_request.code,
            environment: execute_request.environment,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AgentError> {
        let valid_name = !self.workload_name.is_empty()
            && self.workload_name.len() <= MAX_WORKLOAD_NAME_BYTES
            && self
                .workload_name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid_name {
            return Err(AgentError::InvalidWorkloadName);
        }

        if self.code.is_empty()
            || self.code.len() > MAX_SOURCE_BYTES
            || self.config_string.len() > MAX_CONFIG_BYTES
        {
            return Err(AgentError::RequestTooLarge);
        }

        if self.environment.len() > ALLOWED_WORKLOAD_ENVIRONMENT.len() {
            return Err(AgentError::InvalidEnvironment("too many variables".into()));
        }

        for (name, value) in &self.environment {
            if !ALLOWED_WORKLOAD_ENVIRONMENT.contains(&name.as_str())
                || value.len() > MAX_ENV_VALUE_BYTES
                || value.contains('\0')
                || value.contains('\n')
                || value.contains('\r')
            {
                return Err(AgentError::InvalidEnvironment(name.clone()));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Prepare,
    Run,
    PrepareAndRun,
}

impl From<execute_request::Action> for Action {
    fn from(value: execute_request::Action) -> Self {
        match value {
            execute_request::Action::Prepare => Action::Prepare,
            execute_request::Action::Run => Action::Run,
            execute_request::Action::PrepareAndRun => Action::PrepareAndRun,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use crate::agent::ExecuteRequest;
    use std::collections::HashMap;

    fn request(name: &str) -> ExecuteRequest {
        ExecuteRequest {
            workload_name: name.into(),
            language: "rust".into(),
            action: 2,
            code: "fn main() {}".into(),
            config_str: "[build]\nrelease = true".into(),
            environment: HashMap::new(),
        }
    }

    #[test]
    fn rejects_path_like_workload_names() {
        assert!(Config::new_from_execute_request(request("../escape")).is_err());
    }

    #[test]
    fn rejects_build_environment_injection() {
        let mut request = request("safe-name");
        request
            .environment
            .insert("RUSTC_WRAPPER".into(), "/tmp/evil".into());
        assert!(Config::new_from_execute_request(request).is_err());
    }
}
