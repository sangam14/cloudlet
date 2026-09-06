pub mod core;
pub mod llmman;
pub mod security;
pub mod grpc {
    pub mod client;
    pub mod server;
}

use security::VmmServiceConfig;
use tonic::{service::Interceptor, Request, Status};

/// Runs the privileged VMM gRPC control service.
///
/// This is intentionally separate from the desktop/API role even when all
/// roles are dispatched from the same `cloudlet` executable. A service
/// manager can grant this process only KVM/network access while keeping UI and
/// HTTP code unprivileged.
pub async fn serve_grpc(config: VmmServiceConfig) -> Result<(), tonic::transport::Error> {
    let service =
        grpc::server::vmmorchestrator::vmm_service_server::VmmServiceServer::with_interceptor(
            grpc::server::VmmService::new(config.max_concurrent_workloads),
            BrokerAuth::new(config.auth_token),
        );

    tonic::transport::Server::builder()
        .add_service(service)
        .serve(config.address)
        .await
}

#[derive(Clone)]
struct BrokerAuth {
    expected_authorization: Option<String>,
}

impl BrokerAuth {
    fn new(token: Option<String>) -> Self {
        Self {
            expected_authorization: token.map(|token| format!("Bearer {token}")),
        }
    }
}

impl Interceptor for BrokerAuth {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let Some(expected) = self.expected_authorization.as_deref() else {
            return Ok(request);
        };

        let supplied = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok());

        if supplied == Some(expected) {
            Ok(request)
        } else {
            Err(Status::unauthenticated(
                "missing or invalid Cloudlet VMM bearer token",
            ))
        }
    }
}

#[derive(Debug)]
pub enum VmmErrors {
    VmmNew(core::Error),
    VmmConfigure(core::Error),
    VmmRun(core::Error),
    VmmBuildEnvironment(std::io::Error),
}
