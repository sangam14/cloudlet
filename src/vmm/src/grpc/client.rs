use self::agent::{workload_runner_client::WorkloadRunnerClient, ExecuteRequest, SignalRequest};
use super::server::vmmorchestrator::{ShutdownVmRequest, ShutdownVmResponse};
use log::error;
use std::{env, net::Ipv4Addr, time::Duration};
use tonic::{metadata::MetadataValue, transport::Channel, Request, Status, Streaming};

pub mod agent {
    tonic::include_proto!("cloudlet.agent");
}

pub struct WorkloadClient {
    client: WorkloadRunnerClient<Channel>,
    auth_header: Option<MetadataValue<tonic::metadata::Ascii>>,
}

impl WorkloadClient {
    pub async fn new(
        ip: Ipv4Addr,
        port: u16,
        auth_token: Option<&str>,
    ) -> Result<Self, tonic::transport::Error> {
        let delay = Duration::from_secs(2);
        let attempts = env::var("CLOUDLET_AGENT_CONNECT_ATTEMPTS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|attempts| *attempts > 0)
            .unwrap_or(10);
        let auth_header = auth_token.map(|token| {
            format!("Bearer {token}")
                .parse()
                .expect("agent token must be valid gRPC metadata")
        });
        let endpoint = format!("http://[{}]:{}", ip, port);
        let mut last_error = None;

        for attempt in 1..=attempts {
            match WorkloadRunnerClient::connect(endpoint.clone()).await {
                Ok(client) => {
                    return Ok(WorkloadClient {
                        client,
                        auth_header,
                    });
                }
                Err(err) => {
                    error!(
                        "Failed to connect to Agent service (attempt {attempt}/{attempts}): {err}"
                    );
                    last_error = Some(err);
                    if attempt < attempts {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        // The loop always performs at least one connection attempt.
        Err(last_error.expect("agent connection attempts must produce an error"))
    }

    pub async fn execute(
        &mut self,
        request: ExecuteRequest,
    ) -> Result<Streaming<agent::ExecuteResponse>, tonic::Status> {
        let request = self.authorized_request(request)?;
        let response_stream = self.client.execute(request).await?.into_inner();

        Ok(response_stream)
    }

    pub async fn shutdown(
        &mut self,
        _request: ShutdownVmRequest,
    ) -> Result<ShutdownVmResponse, tonic::Status> {
        let signal_request = self.authorized_request(SignalRequest::default())?;
        self.client.signal(signal_request).await?;
        Ok(ShutdownVmResponse { success: true })
    }

    fn authorized_request<T>(&self, message: T) -> Result<Request<T>, Status> {
        let mut request = Request::new(message);
        if let Some(auth_header) = &self.auth_header {
            request
                .metadata_mut()
                .insert("authorization", auth_header.clone());
        }
        Ok(request)
    }
}
