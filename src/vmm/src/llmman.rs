use std::{
    env, io,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Starts a bridge-private llmman daemon when one is not already listening.
pub fn ensure_running(host_ip: Ipv4Addr) -> Result<String, Error> {
    let port = env::var("CLOUDLET_LLMMAN_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(17_434);
    let address = SocketAddr::from((host_ip, port));

    if is_listening(address) {
        return Ok(format!("http://{address}/v1"));
    }

    let binary = env::var("CLOUDLET_LLMMAN_BIN").unwrap_or_else(|_| {
        env::current_dir()
            .ok()
            .map(|directory| directory.join("tools/bin/llmman"))
            .filter(|path| path.is_file())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "llmman".to_string())
    });
    Command::new(&binary)
        .arg("serve")
        // The bridge is reachable by VMs but not by external host interfaces.
        .env("LLMMAN_HOST", address.to_string())
        // Do not hand Cloudlet's internal control credentials to a model
        // router subprocess. Provider credentials remain opt-in through the
        // normal llmman configuration environment.
        .env_remove("CLOUDLET_API_AUTH_TOKEN")
        .env_remove("CLOUDLET_VMM_AUTH_TOKEN")
        .env_remove("CLOUDLET_AGENT_AUTH_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| Error::Start { binary, source })?;

    let timeout = env::var("CLOUDLET_LLMMAN_STARTUP_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(15);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    while Instant::now() < deadline {
        if is_listening(address) {
            return Ok(format!("http://{address}/v1"));
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(Error::Timeout { address, timeout })
}

fn is_listening(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok()
}

#[derive(Debug)]
pub enum Error {
    Start { binary: String, source: io::Error },
    Timeout { address: SocketAddr, timeout: u64 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start { binary, source } if source.kind() == io::ErrorKind::NotFound => write!(
                formatter,
                "{binary} was not found; install llmman or set CLOUDLET_LLMMAN_BIN"
            ),
            Self::Start { binary, source } => {
                write!(formatter, "could not start {binary} serve: {source}")
            }
            Self::Timeout { address, timeout } => write!(
                formatter,
                "llmman did not start on {address} within {timeout} seconds"
            ),
        }
    }
}

impl std::error::Error for Error {}
