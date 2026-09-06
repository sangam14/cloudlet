use crate::args::{CliArgs, Commands};
use clap::Parser;
use tracing::info;
use vmm::{core::vmm::VMM, security::VmmServiceConfig, VmmErrors};
mod args;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the configuration and configure logger verbosity
    let args = CliArgs::parse();

    info!(
        app_name = env!("CARGO_PKG_NAME"),
        app_version = env!("CARGO_PKG_VERSION"),
        "Starting application",
    );

    // check if the args is grpc or command
    match args.command {
        Commands::Grpc => {
            tracing_subscriber::fmt().init();
            let config = VmmServiceConfig::try_from_env()?;
            vmm::serve_grpc(config).await?;
        }
        Commands::Cli(cli_args) => {
            tracing_subscriber::fmt()
                .with_max_level(cli_args.convert_log_to_tracing())
                .init();

            // Create a new VMM
            let mut vmm = VMM::new(
                cli_args.iface_host_addr,
                cli_args.netmask,
                cli_args.iface_guest_addr,
            )
            .map_err(VmmErrors::VmmNew)
            .unwrap();

            vmm.configure(
                cli_args.cpus,
                cli_args.memory,
                cli_args.kernel,
                &cli_args.initramfs,
            )
            .await
            .map_err(VmmErrors::VmmConfigure)
            .unwrap();

            // Run the VMM
            vmm.run().map_err(VmmErrors::VmmRun).unwrap();
        }
    }

    Ok(())
}
