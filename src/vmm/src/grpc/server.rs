use self::vmmorchestrator::{
    vmm_service_server::VmmService as VmmServiceTrait, HealthRequest, HealthResponse, Language,
    RunVmmRequest, ShutdownVmRequest, ShutdownVmResponse,
};
use crate::grpc::client::agent::ExecuteRequest;
use crate::VmmErrors;
use crate::{core::vmm::VMM, grpc::client::WorkloadClient, llmman};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::Write;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{
    convert::From,
    env::current_dir,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info};

type Result<T> = std::result::Result<Response<T>, tonic::Status>;

pub mod vmmorchestrator {
    tonic::include_proto!("vmmorchestrator");
}

pub mod agent {
    tonic::include_proto!("cloudlet.agent");
}

// Implement the From trait for VmmErrors into Status
impl From<VmmErrors> for Status {
    fn from(error: VmmErrors) -> Self {
        // log the gRPC error before sending it
        error!("VMM error: {:?}", error);

        // You can create a custom Status variant based on the error
        match error {
            VmmErrors::VmmNew(_) => Status::internal("Error creating VMM"),
            VmmErrors::VmmConfigure(_) => Status::internal("Error configuring VMM"),
            VmmErrors::VmmRun(_) => Status::internal("Error running VMM"),
            VmmErrors::VmmBuildEnvironment(_) => {
                Status::internal("Error while compiling the necessary files for the VMM")
            }
        }
    }
}

pub struct VmmService {
    workload_slots: Arc<Semaphore>,
    active_guest: Arc<Mutex<Option<ActiveGuest>>>,
}

struct ActiveGuest {
    address: Ipv4Addr,
    agent_auth_token: String,
    // Hold the VMM admission slot until the guest is explicitly shut down.
    // A workload's output stream ending does not mean its VM has stopped.
    _permit: OwnedSemaphorePermit,
}

impl VmmService {
    pub fn new(max_concurrent_workloads: usize) -> Self {
        Self {
            workload_slots: Arc::new(Semaphore::new(max_concurrent_workloads)),
            active_guest: Arc::new(Mutex::new(None)),
        }
    }

    pub fn get_initramfs(
        &self,
        language: &str,
        curr_dir: &OsStr,
    ) -> std::result::Result<PathBuf, VmmErrors> {
        // define initramfs file placement
        let mut initramfs_entire_file_path = curr_dir.to_os_string();
        initramfs_entire_file_path.push(format!("/tools/rootfs/{language}-secure-v1.img"));
        // set image name
        let image = format!("{language}:alpine");

        // check if an initramfs already exists
        let rootfs_exists = Path::new(&initramfs_entire_file_path)
            .try_exists()
            .map_err(VmmErrors::VmmBuildEnvironment)?;
        if !rootfs_exists {
            // build the agent
            let agent_file_name = self.get_path(
                curr_dir,
                "/target/x86_64-unknown-linux-musl/release/agent",
                "cargo",
                vec![
                    "build",
                    "--release",
                    "--bin",
                    "agent",
                    "--target=x86_64-unknown-linux-musl",
                ],
            )?;
            // build initramfs
            info!("Building initramfs");
            let agent_file_name = agent_file_name.to_str().ok_or_else(|| {
                VmmErrors::VmmBuildEnvironment(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "agent path is not valid UTF-8",
                ))
            })?;
            let initramfs_path = initramfs_entire_file_path.to_str().ok_or_else(|| {
                VmmErrors::VmmBuildEnvironment(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "initramfs path is not valid UTF-8",
                ))
            })?;
            self.run_command(
                "sh",
                vec![
                    "./tools/rootfs/mkrootfs.sh",
                    &image,
                    agent_file_name,
                    initramfs_path,
                ],
            )
            .map_err(VmmErrors::VmmBuildEnvironment)?;
        }
        Ok(PathBuf::from(&initramfs_entire_file_path))
    }

    pub fn run_command(
        &self,
        command_type: &str,
        args: Vec<&str>,
    ) -> std::result::Result<(), std::io::Error> {
        // Execute the script using sh and capture output and error streams
        let status = Command::new(command_type)
            .args(args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "{command_type} exited with {status}"
            )))
        }
    }

    pub fn get_path(
        &self,
        curr_dir: &OsStr,
        end_path: &str,
        command_type: &str,
        args: Vec<&str>,
    ) -> std::result::Result<PathBuf, VmmErrors> {
        // define file path
        let mut entire_path = curr_dir.to_os_string();
        entire_path.push(end_path);

        // Check if the file is on the system, else build it
        let exists = Path::new(&entire_path)
            .try_exists()
            .map_err(VmmErrors::VmmBuildEnvironment)?;

        if !exists {
            info!("File {:?} not found, building it", &entire_path);
            self.run_command(command_type, args)
                .map_err(VmmErrors::VmmBuildEnvironment)?;
            if !Path::new(&entire_path)
                .try_exists()
                .map_err(VmmErrors::VmmBuildEnvironment)?
            {
                return Err(VmmErrors::VmmBuildEnvironment(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("build completed but {entire_path:?} was not created"),
                )));
            }
            info!("File {:?} successfully build", &entire_path);
        };
        Ok(PathBuf::from(&entire_path))
    }

    pub fn get_agent_request(
        &self,
        vmm_request: RunVmmRequest,
        language: String,
        llm_base_url: String,
    ) -> ExecuteRequest {
        // Send the grpc request to start the agent
        ExecuteRequest {
            workload_name: vmm_request.workload_name,
            language,
            action: 2, // Prepare and run
            code: vmm_request.code,
            config_str: "[build]\nrelease = true".to_string(),
            environment: HashMap::from([
                ("CLOUDLET_LLM_BASE_URL".to_string(), llm_base_url.clone()),
                ("OPENAI_BASE_URL".to_string(), llm_base_url),
                ("OPENAI_API_KEY".to_string(), "llmman".to_string()),
            ]),
        }
    }
}

impl Default for VmmService {
    fn default() -> Self {
        Self::new(1)
    }
}

#[tonic::async_trait]
impl VmmServiceTrait for VmmService {
    type RunStream =
        ReceiverStream<std::result::Result<vmmorchestrator::ExecuteResponse, tonic::Status>>;

    async fn health(&self, _request: Request<HealthRequest>) -> Result<HealthResponse> {
        Ok(Response::new(HealthResponse { ready: true }))
    }

    async fn shutdown(&self, request: Request<ShutdownVmRequest>) -> Result<ShutdownVmResponse> {
        let (guest_address, agent_auth_token) = {
            let active_guest = self
                .active_guest
                .lock()
                .map_err(|_| Status::internal("active guest registry is unavailable"))?;
            let active_guest = active_guest
                .as_ref()
                .ok_or_else(|| Status::failed_precondition("there is no active Cloudlet guest"))?;
            (active_guest.address, active_guest.agent_auth_token.clone())
        };

        tokio::time::sleep(Duration::from_secs(2)).await;
        let grpc_client = WorkloadClient::new(guest_address, 50051, Some(&agent_auth_token)).await;

        match grpc_client {
            Ok(mut client) => {
                info!("Attempting to stop the active VM workload...");
                let response = client.shutdown(request.into_inner()).await?;
                // Keep the admission slot until the guest halts and the VMM
                // process exits. Clearing it on an RPC acknowledgement would
                // allow another VM to reuse the bridge while the old one was
                // still winding down.
                Ok(Response::new(response))
            }
            Err(error) => {
                error!(%error, "could not reach active guest agent for shutdown");
                Err(Status::unavailable(
                    "failed to reach the active guest agent",
                ))
            }
        }
    }

    async fn run(&self, request: Request<RunVmmRequest>) -> Result<Self::RunStream> {
        let permit = self
            .workload_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                Status::resource_exhausted(
                    "the VMM is at its configured concurrent workload capacity",
                )
            })?;
        let (tx, rx) = tokio::sync::mpsc::channel(4);

        const HOST_IP: Ipv4Addr = Ipv4Addr::new(172, 29, 0, 1);
        // Keep the guest on a narrow private subnet rather than sharing the
        // entire 172.29.0.0/16 range with unrelated host interfaces.
        const HOST_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
        const GUEST_IP: Ipv4Addr = Ipv4Addr::new(172, 29, 0, 2);

        // get current directory
        let curr_dir = current_dir()
            .map_err(VmmErrors::VmmBuildEnvironment)?
            .into_os_string();

        // build kernel if necessary
        let kernel_path: PathBuf = self.get_path(
            &curr_dir,
            "/tools/kernel/linux-cloud-hypervisor/arch/x86/boot/compressed/vmlinux.bin",
            "sh",
            vec!["./tools/kernel/mkkernel.sh"],
        )?;

        // get request with the language
        let vmm_request = request.into_inner();
        validate_run_request(&vmm_request)?;
        let language: String = Language::try_from(vmm_request.language)
            .map_err(|_| Status::invalid_argument("unsupported workload language"))?
            .as_str_name()
            .to_lowercase();

        let initramfs_path = self.get_initramfs(&language, curr_dir.as_os_str())?;
        let agent_auth_token = generate_agent_auth_token()?;
        let boot_parameters = vec![format!("cloudlet.agent-token={agent_auth_token}")];

        let mut vmm = VMM::new(HOST_IP, HOST_NETMASK, GUEST_IP).map_err(VmmErrors::VmmNew)?;

        // Configure the VMM parameters might need to be calculated rather than hardcoded
        vmm.configure_with_boot_parameters(
            1,
            4000,
            kernel_path,
            &Some(initramfs_path),
            boot_parameters,
        )
        .await
        .map_err(VmmErrors::VmmConfigure)?;

        let llm_base_url = llmman::ensure_running(HOST_IP)
            .map_err(|error| Status::failed_precondition(error.to_string()))?;
        info!(%llm_base_url, "llmman is ready for guest workloads");

        // Run the VMM in a separate task
        tokio::spawn(async move {
            info!("Running VMM");
            if let Err(err) = vmm.run().map_err(VmmErrors::VmmRun) {
                error!("Error running VMM: {:?}", err);
            }
        });

        *self
            .active_guest
            .lock()
            .map_err(|_| Status::internal("active guest registry is unavailable"))? =
            Some(ActiveGuest {
                address: GUEST_IP,
                agent_auth_token: agent_auth_token.clone(),
                _permit: permit,
            });

        // run the grpc client
        tokio::time::sleep(Duration::from_secs(2)).await;
        info!("Connecting to Agent service");
        let grpc_client = WorkloadClient::new(GUEST_IP, 50051, Some(&agent_auth_token)).await;

        let agent_request = self.get_agent_request(vmm_request, language, llm_base_url);

        match grpc_client {
            Ok(mut client) => {
                info!("Successfully connected to Agent service");

                // Start the execution
                let mut response_stream = client.execute(agent_request).await?;

                // Process each message as it arrives
                tokio::spawn(async move {
                    while let Ok(Some(response)) = response_stream.message().await {
                        let vmm_response = vmmorchestrator::ExecuteResponse {
                            stage: response.stage,
                            stdout: response.stdout,
                            stderr: response.stderr,
                            exit_code: response.exit_code,
                        };
                        let _ = tx.send(Ok(vmm_response)).await;
                    }
                });
            }
            Err(e) => {
                error!("ERROR {:?}", e);
                let _ = tx
                    .send(Err(Status::unavailable("guest agent did not become ready")))
                    .await;
            }
        }

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

fn validate_run_request(request: &RunVmmRequest) -> std::result::Result<(), Status> {
    let valid_name = !request.workload_name.is_empty()
        && request.workload_name.len() <= 64
        && request
            .workload_name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid_name {
        return Err(Status::invalid_argument(
            "workload_name must be 1-64 lowercase ASCII letters, digits, or hyphens",
        ));
    }

    if request.code.is_empty() || request.code.len() > 240 * 1024 {
        return Err(Status::resource_exhausted(
            "workload source must be between 1 and 245760 bytes",
        ));
    }

    Ok(())
}

fn generate_agent_auth_token() -> std::result::Result<String, Status> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|error| {
            error!(%error, "could not obtain guest agent credentials from the OS random source");
            Status::internal("could not generate a guest agent credential")
        })?;

    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::{validate_run_request, vmmorchestrator::RunVmmRequest};

    fn request(name: &str, code: &str) -> RunVmmRequest {
        RunVmmRequest {
            workload_name: name.into(),
            language: 0,
            code: code.into(),
            log_level: 1,
        }
    }

    #[test]
    fn rejects_path_like_workload_names() {
        assert!(validate_run_request(&request("../escape", "fn main() {}")).is_err());
    }

    #[test]
    fn rejects_oversized_source_before_vm_creation() {
        let source = "x".repeat(240 * 1024 + 1);
        assert!(validate_run_request(&request("safe-name", &source)).is_err());
    }
}
