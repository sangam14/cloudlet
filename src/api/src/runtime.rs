//! BoxLite is the only sandbox runtime, embedded in-process. Untrusted source is only ever
//! passed over guest stdin; it is never interpolated into a host command.
use std::{path::PathBuf, sync::Arc, time::Duration};

use boxlite::{BoxCommand, BoxOptions, BoxliteOptions, BoxliteRuntime, NetworkSpec, RootfsSpec};
use control_plane::{CapacitySummary, Language, WorkloadRequest, WorkloadStatus, WorkloadSummary};
use futures::StreamExt;
use tokio::sync::{mpsc, watch, Mutex, OwnedSemaphorePermit, Semaphore};

use crate::service::{ExecuteJsonResponse, StageJson};

const EXECUTION_TIMEOUT: Duration = Duration::from_secs(60);
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(600);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_BOXES: usize = 100;

pub struct SandboxRuntime {
    runtime: Result<BoxliteRuntime, String>,
    home: PathBuf,
    gate: Arc<Semaphore>,
    cancel: Mutex<Option<watch::Sender<bool>>>,
    cleanup_pending: Mutex<Option<String>>,
    active_id: Mutex<Option<String>>,
}

impl Default for SandboxRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxRuntime {
    pub fn new() -> Self {
        let home = std::env::var_os("CLOUDLET_BOXLITE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let base = std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                            .join(".local/share")
                    });
                base.join("cloudlet/boxlite")
            });
        let runtime = BoxliteRuntime::new(BoxliteOptions { home_dir: home.clone(), ..Default::default() })
            .map_err(|error| format!("BoxLite initialization failed: {error}. On Linux, run Cloudlet from a host terminal with read/write access to /dev/kvm. Restart Cloudlet after fixing host access."));
        Self {
            runtime,
            home,
            gate: Arc::new(Semaphore::new(1)),
            cancel: Mutex::new(None),
            cleanup_pending: Mutex::new(None),
            active_id: Mutex::new(None),
        }
    }

    pub fn ready(&self) -> bool {
        self.runtime.is_ok()
    }

    pub fn detail(&self) -> String {
        match &self.runtime {
            Ok(_) => format!("BoxLite {} initialized; host virtualization check passed. Guest boot is verified on execution. State: {}", boxlite::VERSION, self.home.display()),
            Err(error) => error.clone(),
        }
    }

    fn engine(&self) -> Result<&BoxliteRuntime, String> {
        self.runtime.as_ref().map_err(Clone::clone)
    }

    pub async fn inventory(&self) -> Result<(CapacitySummary, Vec<WorkloadSummary>), String> {
        let boxes = self
            .engine()?
            .list_info()
            .await
            .map_err(|e| e.to_string())?;
        let mut capacity = CapacitySummary {
            running: 0,
            stopped: 0,
            vcpus: 0,
            memory_mib: 0,
        };
        let mut workloads = Vec::new();
        for info in boxes {
            let active = info.status.is_active() || info.pid.is_some();
            if active {
                capacity.running += 1;
                capacity.vcpus += u32::from(info.cpus);
                capacity.memory_mib += info.memory_mib;
            } else {
                capacity.stopped += 1;
            }
            workloads.push(WorkloadSummary {
                id: info.id.to_string(),
                name: info.name.unwrap_or_else(|| info.id.to_string()),
                runtime: info.image,
                status: if active {
                    WorkloadStatus::Running
                } else {
                    WorkloadStatus::Stopped
                },
                updated_at: info.last_updated.to_rfc3339(),
            });
        }
        workloads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok((capacity, workloads))
    }

    /// Reserve before returning an SSE response, so concurrent clients receive
    /// an explicit conflict instead of exceeding the host's resource budget.
    pub fn reserve(&self) -> Result<OwnedSemaphorePermit, String> {
        self.engine()?;
        self.gate.clone().try_acquire_owned().map_err(|_| {
            "Another execution or lifecycle operation is active. Wait or stop it first.".into()
        })
    }

    pub async fn cancel_active(&self) -> bool {
        self.cancel
            .lock()
            .await
            .as_ref()
            .is_some_and(|sender| sender.send(true).is_ok())
    }

    pub async fn stop(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        // A second tab can stop this exact running box. Signal the execution
        // owner, then wait until it has finished cleanup and released admission.
        let owns_execution = self.active_id.lock().await.as_deref() == Some(id);
        let _permit = if owns_execution {
            self.cancel_active().await;
            tokio::time::timeout(Duration::from_secs(35), self.gate.clone().acquire_owned())
                .await
                .map_err(|_| "Execution cleanup is still in progress".to_string())?
                .map_err(|e| e.to_string())?
        } else {
            self.reserve()?
        };
        let litebox = self
            .engine()?
            .get(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Sandbox was not found")?;
        tokio::time::timeout(STOP_TIMEOUT, litebox.stop())
            .await
            .map_err(|_| "BoxLite stop timed out; sandbox state is unknown".to_string())?
            .map_err(|e| e.to_string())?;
        let mut pending = self.cleanup_pending.lock().await;
        if pending.as_deref() == Some(litebox.id().as_str()) {
            *pending = None;
        }
        Ok(())
    }

    pub async fn remove(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        let _permit = self.reserve()?;
        // Never force-delete a live sandbox or its filesystem.
        self.engine()?
            .remove(id, false)
            .await
            .map_err(|e| e.to_string())?;
        let mut pending = self.cleanup_pending.lock().await;
        if pending.as_deref() == Some(id) {
            *pending = None;
        }
        Ok(())
    }

    pub async fn execute(
        self: Arc<Self>,
        request: WorkloadRequest,
        sender: mpsc::Sender<ExecuteJsonResponse>,
        _permit: OwnedSemaphorePermit,
    ) {
        let (cancel, mut cancelled) = watch::channel(false);
        *self.cancel.lock().await = Some(cancel);
        let result = self.execute_inner(request, &sender, &mut cancelled).await;
        *self.cancel.lock().await = None;
        *self.active_id.lock().await = None;
        let event = match result {
            Ok(code) => ExecuteJsonResponse {
                stage: if code == 0 {
                    StageJson::Done
                } else {
                    StageJson::Failed
                },
                stdout: Some("\n[cloudlet] Sandbox stopped.\n".into()),
                stderr: None,
                exit_code: Some(code),
                raw_output: true,
            },
            Err(error) => ExecuteJsonResponse {
                stage: StageJson::Failed,
                stdout: None,
                stderr: Some(format!("\n[cloudlet] {error}\n")),
                exit_code: None,
                raw_output: true,
            },
        };
        // A slow/disconnected browser cannot retain the admission permit forever.
        let _ = tokio::time::timeout(Duration::from_secs(2), sender.send(event)).await;
    }

    async fn execute_inner(
        &self,
        request: WorkloadRequest,
        sender: &mpsc::Sender<ExecuteJsonResponse>,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<i32, String> {
        let runtime = self.engine()?;
        if let Some(id) = self.cleanup_pending.lock().await.as_ref() {
            return Err(format!("Previous cleanup is unconfirmed for {id}. Stop that sandbox explicitly before another execution."));
        }
        let boxes = runtime.list_info().await.map_err(|e| e.to_string())?;
        if boxes.len() >= MAX_BOXES {
            return Err("Sandbox inventory limit reached (100). Remove stopped sandboxes before creating another.".into());
        }
        if boxes
            .iter()
            .any(|info| info.status.is_active() || info.pid.is_some())
        {
            return Err("A sandbox is still active. Stop it before creating another.".into());
        }
        let id = boxlite::BoxIDMint::mint();
        let name = format!("{}-{id}", request.workload_name);
        let litebox = runtime
            .create(options(request.language), Some(name))
            .await
            .map_err(|e| e.to_string())?;
        *self.active_id.lock().await = Some(litebox.id().to_string());
        let work = async {
            send(sender, StageJson::Building, Some(format!("Preparing {} in BoxLite sandbox {} (cold image pulls can take several minutes).\n", image(request.language), litebox.id())), None).await?;
            litebox.start().await.map_err(|e| e.to_string())?;
            send(
                sender,
                StageJson::Running,
                Some("Guest booted. Executing source with a 60-second limit.\n".into()),
                None,
            )
            .await?;
            let mut execution = litebox
                .exec(command(request.language))
                .await
                .map_err(|e| e.to_string())?;
            let mut stdin = execution.stdin().ok_or("Guest stdin unavailable")?;
            let stdout = execution.stdout().ok_or("Guest stdout unavailable")?;
            let stderr = execution.stderr().ok_or("Guest stderr unavailable")?;
            let input = async {
                stdin
                    .write_all(request.code.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                stdin.close();
                Ok::<_, String>(())
            };
            let output = async {
                let mut output = futures::stream::select(
                    stdout.map(|line| (false, line)),
                    stderr.map(|line| (true, line)),
                );
                let mut total = 0usize;
                while let Some((is_error, line)) = output.next().await {
                    total = total.saturating_add(line.len());
                    if total > MAX_OUTPUT_BYTES {
                        return Err("Output exceeded 1 MiB; sandbox stopped.".into());
                    }
                    // Bound each SSE frame, including a guest's very long line.
                    for chunk in chunks(&line, 16 * 1024) {
                        let (out, err) = if is_error {
                            (None, Some(chunk.into()))
                        } else {
                            (Some(chunk.into()), None)
                        };
                        send(sender, StageJson::Running, out, err).await?;
                    }
                }
                Ok::<_, String>(())
            };
            let wait = async { execution.wait().await.map_err(|e| e.to_string()) };
            let (_, _, result) = tokio::try_join!(input, output, wait)?;
            Ok(result.exit_code)
        };
        let result = tokio::select! {
            result = tokio::time::timeout(LIFECYCLE_TIMEOUT, work) => result.map_err(|_| "Sandbox preparation/execution exceeded 10 minutes".to_string()).and_then(|r| r),
            _ = sender.closed() => Err("Console disconnected; sandbox stopped".into()),
            _ = cancelled.changed() => Err("Execution cancelled; sandbox stopped".into()),
        };
        // Cleanup is outside the cancelled/timeout future and completes before
        // emitting a terminal status or releasing the concurrency permit.
        *self.cleanup_pending.lock().await = Some(litebox.id().to_string());
        tokio::time::timeout(STOP_TIMEOUT, litebox.stop())
            .await
            .map_err(|_| {
                "Sandbox stop timed out; check inventory before running again".to_string()
            })?
            .map_err(|e| format!("Sandbox cleanup failed: {e}"))?;
        *self.cleanup_pending.lock().await = None;
        result
    }
}

async fn send(
    sender: &mpsc::Sender<ExecuteJsonResponse>,
    stage: StageJson,
    stdout: Option<String>,
    stderr: Option<String>,
) -> Result<(), String> {
    // Never stall draining the SDK's internal unbounded stdout/stderr queues.
    // Yield between frames so an ordinary output burst cannot starve the HTTP
    // consumer on the same executor; still fail fast if the queue stays full.
    tokio::task::yield_now().await;
    sender
        .try_send(ExecuteJsonResponse {
            stage,
            stdout,
            stderr,
            exit_code: None,
            raw_output: true,
        })
        .map_err(|_| "Console disconnected or is too slow; stopping sandbox".into())
}

fn validate_id(id: &str) -> Result<(), String> {
    if boxlite::BoxID::parse(id).is_some() {
        Ok(())
    } else {
        Err("Expected a complete sandbox ID, not a name or prefix".into())
    }
}

fn image(language: Language) -> &'static str {
    match language {
        Language::Rust => "docker.io/library/rust:1.90-slim-bookworm",
        Language::Python => "docker.io/library/python:3.13-slim-bookworm",
        Language::Node => "docker.io/library/node:22-bookworm-slim",
    }
}

fn options(language: Language) -> BoxOptions {
    BoxOptions {
        cpus: Some(2),
        memory_mib: Some(1024),
        disk_size_gb: Some(8),
        rootfs: RootfsSpec::Image(image(language).into()),
        network: NetworkSpec::Disabled,
        inbound_network: NetworkSpec::Disabled,
        auto_delete: Some(0),
        detach: false,
        // Keep the image's default command from exiting before an exec starts.
        entrypoint: Some(vec!["/bin/sleep".into()]),
        cmd: Some(vec!["infinity".into()]),
        ..Default::default()
    }
}

fn command(language: Language) -> BoxCommand {
    let command = match language {
        // The shell text is constant and runs INSIDE the guest. Source arrives
        // via stdin, so quotes/substitutions in source cannot alter the wrapper.
        Language::Rust => BoxCommand::new("/bin/sh").args(["-c", "set -eu; umask 077; d=$(mktemp -d /tmp/cloudlet.XXXXXX); trap 'rm -rf -- \"$d\"' EXIT; cat > \"$d/main.rs\"; rustc --edition=2024 \"$d/main.rs\" -o \"$d/main\"; \"$d/main\""]),
        Language::Python => BoxCommand::new("python3").args(["-I", "-u", "-"]),
        Language::Node => BoxCommand::new("node").arg("-"),
    };
    command
        .user("65534:65534")
        .working_dir("/tmp")
        .timeout(EXECUTION_TIMEOUT)
}

fn chunks(text: &str, max: usize) -> Vec<&str> {
    let mut rest = text;
    let mut result = Vec::new();
    while rest.len() > max {
        let mut end = max;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        result.push(&rest[..end]);
        rest = &rest[end..];
    }
    result.push(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_never_mount_or_publish_host_resources() {
        for language in [Language::Rust, Language::Python, Language::Node] {
            let options = options(language);
            assert!(options.volumes.is_empty() && options.ports.is_empty());
            assert!(matches!(options.network, NetworkSpec::Disabled));
            assert!(matches!(options.inbound_network, NetworkSpec::Disabled));
            assert!(
                options.advanced.security.jailer_enabled
                    && options.advanced.security.seccomp_enabled
            );
            assert!(!options.advanced.privileged && !options.detach);
            assert_eq!(options.memory_mib, Some(1024));
            assert_eq!(options.cpus, Some(2));
        }
    }
    #[test]
    fn output_frames_preserve_utf8_and_are_bounded() {
        let input = "hello世界".repeat(10000);
        let parts = chunks(&input, 16384);
        assert!(parts.iter().all(|part| part.len() <= 16384));
        assert_eq!(parts.concat(), input);
    }
}
