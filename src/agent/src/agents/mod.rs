use crate::{
    agent::{execute_response::Stage, ExecuteResponse},
    AgentError, AgentResult,
};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[cfg(feature = "debug-agent")]
pub mod debug;
pub mod rust;

#[derive(Debug, Clone)]
pub struct AgentOutput {
    pub stage: Stage,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
}

impl From<AgentOutput> for ExecuteResponse {
    fn from(value: AgentOutput) -> Self {
        Self {
            stage: value.stage as i32,
            stdout: value.stdout,
            stderr: value.stderr,
            exit_code: value.exit_code,
        }
    }
}

#[async_trait]
pub trait Agent {
    async fn prepare(
        &self,
        child_processes: Arc<Mutex<HashSet<u32>>>,
    ) -> AgentResult<mpsc::Receiver<AgentOutput>>;
    async fn run(
        &self,
        child_processes: Arc<Mutex<HashSet<u32>>>,
    ) -> AgentResult<mpsc::Receiver<AgentOutput>>;
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    Rust,
    #[cfg(feature = "debug-agent")]
    Debug,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Rust => write!(f, "rust"),
            #[cfg(feature = "debug-agent")]
            Language::Debug => write!(f, "debug"),
        }
    }
}

impl TryFrom<&str> for Language {
    type Error = AgentError;

    fn try_from(value: &str) -> Result<Self, AgentError> {
        match value {
            "rust" => Ok(Language::Rust),
            #[cfg(feature = "debug-agent")]
            "debug" => Ok(Language::Debug),
            _ => Err(AgentError::InvalidLanguage(format!(
                "Invalid language: {}",
                value
            ))),
        }
    }
}

mod process_utils {
    use super::AgentOutput;
    use crate::agent::execute_response::Stage;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        process::{ChildStderr, ChildStdout},
        sync::mpsc,
        task::JoinHandle,
    };

    const MAX_STREAM_BYTES: usize = 64 * 1024;

    /// Spawn a tokio thread and send each line of `stdout`` to the `tx` given as a parameter.
    pub async fn send_stdout_to_tx(
        stdout: ChildStdout,
        tx: mpsc::Sender<AgentOutput>,
        stage: Option<Stage>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut reader_lines = reader.lines();
            let mut sent_bytes: usize = 0;
            let mut truncated = false;

            while let Ok(Some(line)) = reader_lines.next_line().await {
                if sent_bytes.saturating_add(line.len()) > MAX_STREAM_BYTES {
                    if !truncated {
                        let _ = tx
                            .send(AgentOutput {
                                stage: stage.unwrap_or(Stage::Running),
                                stdout: Some("[Cloudlet output truncated at 64 KiB]".into()),
                                stderr: None,
                                exit_code: None,
                            })
                            .await;
                        truncated = true;
                    }
                    continue;
                }
                sent_bytes += line.len();
                let _ = tx
                    .send(AgentOutput {
                        stage: stage.unwrap_or(Stage::Running),
                        stdout: Some(line),
                        stderr: None,
                        exit_code: None,
                    })
                    .await;
            }
        })
    }

    /// Same as [`send_stdout_to_tx`].
    pub async fn send_stderr_to_tx(
        stderr: ChildStderr,
        tx: mpsc::Sender<AgentOutput>,
        stage: Option<Stage>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut reader_lines = reader.lines();
            let mut sent_bytes: usize = 0;
            let mut truncated = false;

            while let Ok(Some(line)) = reader_lines.next_line().await {
                if sent_bytes.saturating_add(line.len()) > MAX_STREAM_BYTES {
                    if !truncated {
                        let _ = tx
                            .send(AgentOutput {
                                stage: stage.unwrap_or(Stage::Running),
                                stdout: None,
                                stderr: Some("[Cloudlet output truncated at 64 KiB]".into()),
                                exit_code: None,
                            })
                            .await;
                        truncated = true;
                    }
                    continue;
                }
                sent_bytes += line.len();
                let _ = tx
                    .send(AgentOutput {
                        stage: stage.unwrap_or(Stage::Running),
                        stdout: Some(line),
                        stderr: None,
                        exit_code: None,
                    })
                    .await;
            }
        })
    }

    /// Function to wait for the `child` to finish and send the result to the `tx` given as a parameter.
    pub async fn send_exit_status_to_tx(
        mut child: tokio::process::Child,
        tx: mpsc::Sender<AgentOutput>,
        send_done: bool,
        timeout: std::time::Duration,
    ) -> Result<(), ()> {
        let exit_status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(result) => result.map(|status| status.code()),
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = tx
                    .send(AgentOutput {
                        stage: Stage::Failed,
                        stdout: None,
                        stderr: Some(format!(
                            "workload exceeded its {} second execution limit",
                            timeout.as_secs()
                        )),
                        exit_code: None,
                    })
                    .await;
                return Err(());
            }
        };

        match exit_status {
            Ok(exit_code) => {
                if exit_code != Some(0_i32) {
                    let _ = tx
                        .send(AgentOutput {
                            stage: Stage::Failed,
                            stdout: None,
                            stderr: None,
                            exit_code,
                        })
                        .await;

                    Err(())
                } else {
                    if send_done {
                        let _ = tx
                            .send(AgentOutput {
                                stage: Stage::Done,
                                stdout: None,
                                stderr: None,
                                exit_code,
                            })
                            .await;
                    }

                    Ok(())
                }
            }
            Err(e) => {
                let _ = tx
                    .send(AgentOutput {
                        stage: Stage::Failed,
                        stdout: None,
                        stderr: Some(e.to_string()),
                        exit_code: None,
                    })
                    .await;

                Err(())
            }
        }
    }
}
