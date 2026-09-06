//! Shared contracts at the Cloudlet control-plane boundary.
//!
//! This crate intentionally contains only serialisable data. The API, CLI, and
//! dashboard can evolve independently of the privileged VMM implementation.

use std::path::PathBuf;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    Node,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TomlClientConfigFile {
    #[serde(rename = "workload-name")]
    pub workload_name: String,
    pub language: Language,
    pub code_path: PathBuf,
    pub log_level: LogLevel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkloadRequest {
    pub workload_name: String,
    pub language: Language,
    pub code: String,
    pub log_level: LogLevel,
    pub action: String,
    pub server: ServerConfig,
    pub build: BuildConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub address: String,
    pub port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildConfig {
    #[serde(rename = "source-code-path")]
    pub source_code_path: PathBuf,
    pub release: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub service: String,
    pub status: ServiceStatus,
    pub vmm_endpoint: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Ready,
    Degraded,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub connection: ConnectionSummary,
    pub capacity: CapacitySummary,
    pub workloads: Vec<WorkloadSummary>,
    pub models: Vec<ModelSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionSummary {
    pub label: String,
    pub endpoint: String,
    pub status: ServiceStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapacitySummary {
    pub running: u32,
    pub stopped: u32,
    pub vcpus: u32,
    pub memory_mib: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkloadSummary {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub status: WorkloadStatus,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadStatus {
    Running,
    Stopped,
    Building,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelSummary {
    pub name: String,
    pub provider: String,
    pub status: ModelStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    Ready,
    Connect,
    Start,
}
