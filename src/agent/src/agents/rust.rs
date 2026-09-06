use super::{Agent, AgentOutput};
use crate::agent::execute_response::Stage;
use crate::agents::process_utils;
use crate::{workload, AgentError, AgentResult};
use async_trait::async_trait;
use nix::unistd::{chown, Gid, Uid};
use rand::distr::{Alphanumeric, SampleString};
use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::fs::{self, create_dir, create_dir_all, set_permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::{
    mpsc::{self, Receiver},
    watch, Mutex,
};

const WORKLOAD_ROOT: &str = "/tmp/cloudlet-workloads";
const WORKLOAD_DIRECTORY_ATTEMPTS: usize = 32;
const UNPRIVILEGED_WORKLOAD_UID: u32 = 65_534;
const UNPRIVILEGED_WORKLOAD_GID: u32 = 65_534;

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RustAgentBuildConfig {
    release: bool,
}

#[derive(Deserialize)]
struct RustAgentConfig {
    build: RustAgentBuildConfig,
}

pub struct RustAgent {
    workload_config: workload::config::Config,
    rust_config: RustAgentConfig,
    build_notifier: watch::Sender<Option<Result<(), ()>>>,
    workload_dir: Arc<Mutex<Option<PathBuf>>>,
    workload_identity: Option<WorkloadIdentity>,
}

#[derive(Clone, Copy)]
struct WorkloadIdentity {
    uid: u32,
    gid: u32,
}

impl TryFrom<workload::config::Config> for RustAgent {
    type Error = AgentError;

    fn try_from(workload_config: workload::config::Config) -> Result<Self, Self::Error> {
        let rust_config: RustAgentConfig =
            toml::from_str(&workload_config.config_string).map_err(AgentError::ParseConfigError)?;

        Ok(Self {
            workload_config,
            rust_config,
            build_notifier: watch::channel(None).0,
            workload_dir: Arc::new(Mutex::new(None)),
            workload_identity: unprivileged_workload_identity(),
        })
    }
}

impl RustAgent {
    async fn get_build_child_process(
        &self,
        function_dir: &Path,
        child_processes: Arc<Mutex<HashSet<u32>>>,
    ) -> AgentResult<Child> {
        let mut command = Command::new("cargo");
        command
            .env_clear()
            .env("HOME", "/tmp")
            .stderr(Stdio::piped())
            .arg("build")
            .current_dir(function_dir);
        inherit_build_toolchain_environment(&mut command);
        if let Some(identity) = self.workload_identity {
            command.uid(identity.uid).gid(identity.gid);
        }
        if self.rust_config.build.release {
            command.arg("--release");
        }
        let child = command.spawn().map_err(AgentError::WorkloadSetup)?;

        if let Some(child_id) = child.id() {
            child_processes.lock().await.insert(child_id);
        }

        Ok(child)
    }

    async fn prepared_directory(&self) -> AgentResult<PathBuf> {
        self.workload_dir
            .lock()
            .await
            .clone()
            .ok_or(AgentError::WorkloadArtifact)
    }

    async fn wait_for_build(&self) -> AgentResult<()> {
        let mut receiver = self.build_notifier.subscribe();
        let current_result = { *receiver.borrow() };
        let build_result = match current_result {
            Some(result) => result,
            None => {
                receiver
                    .changed()
                    .await
                    .map_err(|_| AgentError::BuildNotifier)?;
                { *receiver.borrow() }.ok_or(AgentError::BuildNotifier)?
            }
        };

        build_result.map_err(|_| AgentError::BuildFailed)
    }
}

#[async_trait]
impl Agent for RustAgent {
    async fn prepare(
        &self,
        child_processes: Arc<Mutex<HashSet<u32>>>,
    ) -> AgentResult<Receiver<AgentOutput>> {
        let function_dir = create_secure_workload_dir().map_err(AgentError::WorkloadSetup)?;
        create_dir_all(function_dir.join("src")).map_err(AgentError::WorkloadSetup)?;
        set_permissions(function_dir.join("src"), fs::Permissions::from_mode(0o700))
            .map_err(AgentError::WorkloadSetup)?;

        write_private_file(
            &function_dir.join("src/main.rs"),
            self.workload_config.code.as_bytes(),
        )
        .map_err(AgentError::WorkloadSetup)?;

        let cargo_toml = format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            self.workload_config.workload_name
        );
        write_private_file(&function_dir.join("Cargo.toml"), cargo_toml.as_bytes())
            .map_err(AgentError::WorkloadSetup)?;
        if let Some(identity) = self.workload_identity {
            make_workload_directory_owned_by(&function_dir, identity)
                .map_err(AgentError::WorkloadSetup)?;
        }

        *self.workload_dir.lock().await = Some(function_dir.clone());
        let mut child = self
            .get_build_child_process(&function_dir, Arc::clone(&child_processes))
            .await?;
        let child_id = child.id();
        let workload_name = self.workload_config.workload_name.clone();
        let is_release = self.rust_config.build.release;
        let tx_build_notifier = self.build_notifier.clone();
        let build_timeout = timeout_from_env("CLOUDLET_BUILD_TIMEOUT_SECS", 120);

        let (tx, rx) = mpsc::channel(10);
        tokio::spawn(async move {
            if let Some(stderr) = child.stderr.take() {
                let _ = process_utils::send_stderr_to_tx(stderr, tx.clone(), Some(Stage::Building))
                    .await
                    .await;
            }

            let build_result =
                process_utils::send_exit_status_to_tx(child, tx.clone(), false, build_timeout)
                    .await;
            if let Some(child_id) = child_id {
                child_processes.lock().await.remove(&child_id);
            }

            let binary_path = if is_release {
                function_dir.join("target/release").join(workload_name)
            } else {
                function_dir.join("target/debug").join(workload_name)
            };
            let build_succeeded = build_result.is_ok() && binary_path.is_file();
            if !build_succeeded {
                if build_result.is_ok() {
                    let _ = tx
                        .send(AgentOutput {
                            stage: Stage::Failed,
                            stdout: None,
                            stderr: Some(
                                "cargo completed but did not produce the workload binary".into(),
                            ),
                            exit_code: None,
                        })
                        .await;
                }
                let _ = fs::remove_dir_all(&function_dir);
                let _ = tx_build_notifier.send(Some(Err(())));
                return;
            }

            let _ = tx_build_notifier.send(Some(Ok(())));
        });

        Ok(rx)
    }

    async fn run(
        &self,
        child_processes: Arc<Mutex<HashSet<u32>>>,
    ) -> AgentResult<Receiver<AgentOutput>> {
        self.wait_for_build().await?;
        let function_dir = self.prepared_directory().await?;
        let binary_path = if self.rust_config.build.release {
            function_dir
                .join("target/release")
                .join(&self.workload_config.workload_name)
        } else {
            function_dir
                .join("target/debug")
                .join(&self.workload_config.workload_name)
        };
        if !binary_path.is_file() {
            return Err(AgentError::WorkloadArtifact);
        }

        let mut command = Command::new(binary_path);
        command
            .env_clear()
            .envs(&self.workload_config.environment)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(identity) = self.workload_identity {
            command.uid(identity.uid).gid(identity.gid);
        }
        let mut child = command.spawn().map_err(AgentError::WorkloadSetup)?;
        let child_id = child.id();
        if let Some(child_id) = child_id {
            child_processes.lock().await.insert(child_id);
        }

        let (tx, rx) = mpsc::channel(10);
        let child_stdout = child.stdout.take().ok_or(AgentError::WorkloadArtifact)?;
        let tx_stdout = tx.clone();
        let child_stderr = child.stderr.take().ok_or(AgentError::WorkloadArtifact)?;
        let tx_stderr = tx;
        let workload_timeout = timeout_from_env("CLOUDLET_WORKLOAD_TIMEOUT_SECS", 30);

        let function_dir_for_cleanup = function_dir.clone();
        tokio::spawn(async move {
            let _ = process_utils::send_stdout_to_tx(child_stdout, tx_stdout.clone(), None)
                .await
                .await;
            let _ = process_utils::send_exit_status_to_tx(child, tx_stdout, true, workload_timeout)
                .await;
            if let Some(child_id) = child_id {
                child_processes.lock().await.remove(&child_id);
            }
            let _ = fs::remove_dir_all(function_dir_for_cleanup);
        });

        tokio::spawn(async move {
            let _ = process_utils::send_stderr_to_tx(child_stderr, tx_stderr, None)
                .await
                .await;
        });

        Ok(rx)
    }
}

fn create_secure_workload_dir() -> std::io::Result<PathBuf> {
    let root = Path::new(WORKLOAD_ROOT);
    create_dir_all(root)?;
    // Other guest users may traverse this root to reach their own random,
    // mode-0700 directory, but cannot list its entries.
    set_permissions(root, fs::Permissions::from_mode(0o711))?;

    for _ in 0..WORKLOAD_DIRECTORY_ATTEMPTS {
        let name = Alphanumeric.sample_string(&mut rand::rng(), 24);
        let directory = root.join(name);
        match create_dir(&directory) {
            Ok(()) => {
                set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
                return Ok(directory);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique Cloudlet workload directory",
    ))
}

fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    fs::write(path, contents)?;
    set_permissions(path, fs::Permissions::from_mode(0o600))
}

fn timeout_from_env(name: &str, default_seconds: u64) -> Duration {
    let seconds = env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=600).contains(seconds))
        .unwrap_or(default_seconds);
    Duration::from_secs(seconds)
}

fn unprivileged_workload_identity() -> Option<WorkloadIdentity> {
    Uid::effective().is_root().then_some(WorkloadIdentity {
        uid: UNPRIVILEGED_WORKLOAD_UID,
        gid: UNPRIVILEGED_WORKLOAD_GID,
    })
}

fn make_workload_directory_owned_by(
    directory: &Path,
    identity: WorkloadIdentity,
) -> std::io::Result<()> {
    let uid = Uid::from_raw(identity.uid);
    let gid = Gid::from_raw(identity.gid);
    for path in [
        directory.join("src/main.rs"),
        directory.join("Cargo.toml"),
        directory.join("src"),
        directory.to_path_buf(),
    ] {
        chown(&path, Some(uid), Some(gid))
            .map_err(|error| std::io::Error::other(error.to_string()))?;
    }
    Ok(())
}

fn inherit_build_toolchain_environment(command: &mut Command) {
    for name in ["CARGO_HOME", "RUSTUP_HOME", "RUST_VERSION", "PATH"] {
        if let Ok(value) = env::var(name) {
            command.env(name, value);
        }
    }
}
