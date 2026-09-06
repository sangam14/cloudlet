use agent::{
    agent::workload_runner_server::WorkloadRunnerServer, workload::service::WorkloadRunnerService,
};
use clap::Parser;
use std::{env, net::ToSocketAddrs};
use tonic::transport::Server;

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, env, default_value = "0.0.0.0")]
    grpc_server_address: String,
    #[clap(long, env, default_value = "50051")]
    grpc_server_port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let bind_address = format!("{}:{}", args.grpc_server_address, args.grpc_server_port)
        .to_socket_addrs()
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid guest agent bind address: {error}"),
            )
        })?
        .next()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "guest agent bind address resolved to no sockets",
            )
        })?;

    let auth_token = env::var("CLOUDLET_AGENT_AUTH_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    if !bind_address.ip().is_loopback() && auth_token.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unauthenticated guest agent network bind; set CLOUDLET_AGENT_AUTH_TOKEN or bind to loopback",
        )
        .into());
    }

    let server = WorkloadRunnerService::new(auth_token);

    Server::builder()
        .add_service(WorkloadRunnerServer::new(server))
        .serve(bind_address)
        .await?;

    Ok(())
}
