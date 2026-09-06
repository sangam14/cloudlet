use std::fmt;

mod agents;
pub mod workload;

#[derive(Debug)]
pub enum AgentError {
    OpenConfigFileError(std::io::Error),
    ParseConfigError(toml::de::Error),
    InvalidLanguage(String),
    InvalidWorkloadName,
    RequestTooLarge,
    InvalidEnvironment(String),
    WorkloadArtifact,
    WorkloadSetup(std::io::Error),
    BuildNotifier,
    BuildFailed,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::OpenConfigFileError(e) => write!(f, "Failed to open config file: {}", e),
            AgentError::ParseConfigError(e) => write!(f, "Failed to parse config file: {}", e),
            AgentError::InvalidLanguage(e) => write!(f, "Invalid language: {}", e),
            AgentError::InvalidWorkloadName => write!(
                f,
                "workload name must be 1-64 lowercase ASCII letters, digits, or hyphens"
            ),
            AgentError::RequestTooLarge => {
                write!(f, "workload request exceeds the guest agent safety limit")
            }
            AgentError::InvalidEnvironment(name) => {
                write!(f, "environment variable is not permitted: {name}")
            }
            AgentError::WorkloadArtifact => {
                write!(f, "prepared workload artifact is unavailable")
            }
            AgentError::WorkloadSetup(error) => {
                write!(f, "could not prepare isolated workload directory: {error}")
            }
            AgentError::BuildNotifier => {
                write!(f, "Could not get notification from build notifier")
            }
            AgentError::BuildFailed => write!(f, "Build has failed"),
        }
    }
}

pub type AgentResult<T> = Result<T, AgentError>;

pub mod agent {
    tonic::include_proto!("cloudlet.agent");
}
