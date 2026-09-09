//! One distributed executable: React console, Rust API, and BoxLite runtime.
//! BoxLite extracts its embedded helper assets and uses isolated subprocesses.
//! BoxLite is the only sandbox runtime.

use std::{env, error::Error, process};

use api::config::ApiConfig;
use tracing::{error, info};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    if let Err(error) = run() {
        error!(%error, "Cloudlet stopped");
        eprintln!("Cloudlet: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    match Role::from_args()? {
        Role::Dashboard | Role::Api => {
            let config = ApiConfig::try_from_env()?;
            info!(host = %config.bind_host, port = config.bind_port, "starting Cloudlet API role");
            api::serve_blocking(config)?;
            Ok(())
        }
        Role::Doctor => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let sandbox = api::runtime::SandboxRuntime::new();
                println!("{}", sandbox.detail());
                if sandbox.ready() {
                    Ok(())
                } else {
                    Err(std::io::Error::other("BoxLite is not ready").into())
                }
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Dashboard,
    Api,
    Doctor,
}

impl Role {
    fn from_args() -> Result<Self, Box<dyn Error>> {
        let argument = env::args().nth(1);
        match argument.as_deref() {
            None | Some("dashboard") | Some("desktop") => Ok(Self::Dashboard),
            Some("api") => Ok(Self::Api),
            Some("doctor") => Ok(Self::Doctor),
            Some("-h") | Some("--help") | Some("help") => {
                println!(
                    "Cloudlet + BoxLite\n\nUSAGE:\n  cloudlet [dashboard|api|doctor]\n\nROLES:\n  dashboard    Embedded shadcn console, Rust API, and BoxLite runtime (default).\n  desktop      Compatibility alias for dashboard.\n  api          Same local HTTP API and embedded console.\n  doctor       Check BoxLite initialization and host virtualization.\n\nBoxLite is the only sandbox runtime. Linux needs read/write /dev/kvm access.\nRun as your normal user, without sudo or CAP_NET_ADMIN.\nNo Node.js, separate broker, or frontend files needed at runtime. Helpers and OCI disks are extracted locally.\nSet CLOUDLET_BOXLITE_HOME for runtime state; default is $XDG_DATA_HOME/cloudlet/boxlite (or ~/.local/share/cloudlet/boxlite)."
                );
                process::exit(0);
            }
            Some(value) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "unknown Cloudlet role {value:?}; run `cloudlet --help` for available roles"
                ),
            )
            .into()),
        }
    }
}
