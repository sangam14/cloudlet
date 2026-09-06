use super::runner::Runner;
use crate::agent::{self, ExecuteRequest, ExecuteResponse, SignalRequest};
use agent::workload_runner_server::WorkloadRunner;
use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

type Result<T> = std::result::Result<Response<T>, tonic::Status>;

static CHILD_PROCESSES: Lazy<Arc<Mutex<HashSet<u32>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashSet::new())));

pub struct WorkloadRunnerService {
    expected_authorization: Option<Arc<str>>,
}

impl WorkloadRunnerService {
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            expected_authorization: auth_token.map(|token| Arc::from(format!("Bearer {token}"))),
        }
    }

    fn authorize<T>(&self, request: &Request<T>) -> std::result::Result<(), Status> {
        let Some(expected) = self.expected_authorization.as_deref() else {
            return Ok(());
        };

        let supplied = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        if supplied == Some(expected) {
            Ok(())
        } else {
            Err(Status::unauthenticated(
                "missing or invalid Cloudlet guest agent bearer token",
            ))
        }
    }
}

#[tonic::async_trait]
impl WorkloadRunner for WorkloadRunnerService {
    type ExecuteStream = ReceiverStream<std::result::Result<ExecuteResponse, tonic::Status>>;

    async fn execute(&self, req: Request<ExecuteRequest>) -> Result<Self::ExecuteStream> {
        self.authorize(&req)?;
        let runner = Runner::new_from_execute_request(req.into_inner(), CHILD_PROCESSES.clone())
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        let mut runner_rx = runner
            .run()
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let (tx, rx) = mpsc::channel(10);
        tokio::spawn(async move {
            while let Some(agent_output) = runner_rx.recv().await {
                println!("Sending to the gRPC client: {:?}", agent_output);
                let _ = tx.send(Ok(agent_output.into())).await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn signal(&self, request: Request<SignalRequest>) -> Result<()> {
        self.authorize(&request)?;
        let child_processes: Vec<u32> = CHILD_PROCESSES.lock().await.iter().copied().collect();

        for child_id in child_processes {
            match nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(child_id as i32),
                nix::sys::signal::Signal::SIGTERM,
            ) {
                Ok(_) => println!("Sent SIGTERM to child process {}", child_id),
                Err(e) => println!(
                    "Failed to send SIGTERM to child process {}: {}",
                    child_id, e
                ),
            }
        }

        // Let the gRPC response flush before halting the guest. The host VMM
        // treats a guest halt as the end of this sandbox's lifetime.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let error = nix::sys::reboot::reboot(nix::sys::reboot::RebootMode::RB_POWER_OFF)
                .expect_err("reboot only returns when the kernel reports an error");
            eprintln!("Cloudlet guest poweroff failed: {error}");
        });

        Ok(Response::new(()))
    }
}
