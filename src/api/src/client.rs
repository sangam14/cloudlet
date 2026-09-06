use std::time::Duration;

use tonic::{
    metadata::MetadataValue,
    transport::{Channel, Endpoint},
    Request, Status, Streaming,
};
use vmmorchestrator::vmm_service_client::VmmServiceClient;

pub mod vmmorchestrator {
    tonic::include_proto!("vmmorchestrator");
}

pub struct VmmClient {
    client: VmmServiceClient<Channel>,
    auth_header: Option<MetadataValue<tonic::metadata::Ascii>>,
}

impl VmmClient {
    pub async fn connect(
        endpoint: &str,
        auth_token: Option<&str>,
    ) -> Result<Self, tonic::transport::Error> {
        let channel = Endpoint::new(endpoint.to_owned())?
            .connect_timeout(Duration::from_secs(2))
            .connect()
            .await?;
        let client = VmmServiceClient::new(channel);
        let auth_header = auth_token.map(|token| {
            format!("Bearer {token}")
                .parse()
                .expect("API configuration validates bearer token metadata")
        });

        Ok(VmmClient {
            client,
            auth_header,
        })
    }

    pub async fn run_vmm(
        &mut self,
        request: vmmorchestrator::RunVmmRequest,
    ) -> Result<Streaming<vmmorchestrator::ExecuteResponse>, tonic::Status> {
        let mut request = self.authorized_request(request)?;
        request.set_timeout(Duration::from_secs(120));
        let response_stream = self.client.run(request).await?.into_inner();

        Ok(response_stream)
    }

    pub async fn health(&mut self) -> Result<vmmorchestrator::HealthResponse, tonic::Status> {
        let mut request = self.authorized_request(vmmorchestrator::HealthRequest {})?;
        request.set_timeout(Duration::from_secs(2));
        Ok(self.client.health(request).await?.into_inner())
    }

    pub async fn shutdown_vm(
        &mut self,
        request: vmmorchestrator::ShutdownVmRequest,
    ) -> Result<vmmorchestrator::ShutdownVmResponse, tonic::Status> {
        let mut request = self.authorized_request(request)?;
        request.set_timeout(Duration::from_secs(5));
        let response = self.client.shutdown(request).await?.into_inner();

        Ok(response)
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
