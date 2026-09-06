//! Host-side security policy for the privileged VMM broker.
//!
//! The VMM has access to `/dev/kvm` and may create TAP/bridge devices. Keep
//! its network control plane local by default and make every remote exposure a
//! deliberate, authenticated choice.

use std::{env, fmt, net::SocketAddr};

pub const DEFAULT_VMM_GRPC_ADDR: &str = "[::1]:50051";
/// The current VMM protocol has a single active-guest shutdown target. Keep
/// admission serial until the lifecycle registry and per-VM network namespaces
/// can safely support multiple concurrent guests.
pub const DEFAULT_MAX_CONCURRENT_WORKLOADS: usize = 1;
pub const MAX_CONCURRENT_WORKLOADS: usize = 1;

#[derive(Clone)]
pub struct VmmServiceConfig {
    pub address: SocketAddr,
    pub auth_token: Option<String>,
    pub max_concurrent_workloads: usize,
}

impl fmt::Debug for VmmServiceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VmmServiceConfig")
            .field("address", &self.address)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[redacted]"),
            )
            .field("max_concurrent_workloads", &self.max_concurrent_workloads)
            .finish()
    }
}

impl VmmServiceConfig {
    pub fn try_from_env() -> Result<Self, ConfigError> {
        let address = env::var("CLOUDLET_VMM_GRPC_ADDR")
            .unwrap_or_else(|_| DEFAULT_VMM_GRPC_ADDR.into())
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::InvalidAddress)?;
        let auth_token = optional_token("CLOUDLET_VMM_AUTH_TOKEN")?;

        if !address.ip().is_loopback() {
            if !bool_env("CLOUDLET_VMM_ALLOW_REMOTE") {
                return Err(ConfigError::RemoteBindRequiresOptIn(address));
            }
            if auth_token.is_none() {
                return Err(ConfigError::RemoteBindRequiresToken);
            }
        } else if auth_token.is_none() && !bool_env("CLOUDLET_VMM_ALLOW_UNAUTHENTICATED_LOCAL") {
            return Err(ConfigError::LocalBindRequiresToken);
        }

        let max_concurrent_workloads = env::var("CLOUDLET_VMM_MAX_CONCURRENT_WORKLOADS")
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| ConfigError::InvalidConcurrency(value))
            })
            .transpose()?
            .unwrap_or(DEFAULT_MAX_CONCURRENT_WORKLOADS);
        if !(1..=MAX_CONCURRENT_WORKLOADS).contains(&max_concurrent_workloads) {
            return Err(ConfigError::ConcurrencyOutOfRange(max_concurrent_workloads));
        }

        Ok(Self {
            address,
            auth_token,
            max_concurrent_workloads,
        })
    }
}

fn optional_token(name: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(name) {
        Ok(value) => {
            if value.len() < 16 || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
                return Err(ConfigError::InvalidToken(name));
            }
            Ok(Some(value))
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidToken(name)),
    }
}

fn bool_env(name: &str) -> bool {
    matches!(
        env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidAddress,
    InvalidToken(&'static str),
    InvalidConcurrency(String),
    ConcurrencyOutOfRange(usize),
    RemoteBindRequiresOptIn(SocketAddr),
    RemoteBindRequiresToken,
    LocalBindRequiresToken,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress => write!(formatter, "invalid CLOUDLET_VMM_GRPC_ADDR"),
            Self::InvalidToken(name) => write!(
                formatter,
                "{name} must contain at least 16 printable, non-space ASCII characters"
            ),
            Self::InvalidConcurrency(value) => write!(
                formatter,
                "invalid CLOUDLET_VMM_MAX_CONCURRENT_WORKLOADS: {value}"
            ),
            Self::ConcurrencyOutOfRange(value) => write!(
                formatter,
                "CLOUDLET_VMM_MAX_CONCURRENT_WORKLOADS must be between 1 and {MAX_CONCURRENT_WORKLOADS}, got {value}"
            ),
            Self::RemoteBindRequiresOptIn(address) => write!(
                formatter,
                "refusing non-loopback VMM bind {address}; set CLOUDLET_VMM_ALLOW_REMOTE=true and CLOUDLET_VMM_AUTH_TOKEN to opt in"
            ),
            Self::RemoteBindRequiresToken => write!(
                formatter,
                "a non-loopback VMM bind requires CLOUDLET_VMM_AUTH_TOKEN"
            ),
            Self::LocalBindRequiresToken => write!(
                formatter,
                "CLOUDLET_VMM_AUTH_TOKEN is required; set CLOUDLET_VMM_ALLOW_UNAUTHENTICATED_LOCAL=true only for local development"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MAX_CONCURRENT_WORKLOADS, DEFAULT_VMM_GRPC_ADDR};

    #[test]
    fn defaults_are_local_and_serial() {
        assert_eq!(DEFAULT_VMM_GRPC_ADDR, "[::1]:50051");
        assert_eq!(DEFAULT_MAX_CONCURRENT_WORKLOADS, 1);
    }
}
