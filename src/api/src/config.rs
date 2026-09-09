use std::{env, fmt, net::IpAddr};

/// Upper bound for a source payload accepted at the public HTTP boundary.
///
/// BoxLite compiles source inside a guest, so accepting arbitrarily large
/// payloads here would let a caller exhaust host memory before a VM is even
/// created.
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct ApiConfig {
    pub bind_host: String,
    pub bind_port: u16,
    /// Optional bearer token required by HTTP control-plane endpoints.
    pub auth_token: Option<String>,
    pub max_request_bytes: usize,
}

impl fmt::Debug for ApiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiConfig")
            .field("bind_host", &self.bind_host)
            .field("bind_port", &self.bind_port)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[redacted]"),
            )
            .field("max_request_bytes", &self.max_request_bytes)
            .finish()
    }
}

impl ApiConfig {
    /// Reads a deliberately conservative control-plane configuration.
    ///
    /// Non-loopback binds must be explicitly enabled and authenticated. This
    /// prevents an environment typo from accidentally publishing arbitrary
    /// workload execution on a LAN interface.
    pub fn try_from_env() -> Result<Self, ConfigError> {
        let bind_host = env::var("CLOUDLET_API_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let bind_port = env::var("CLOUDLET_API_PORT")
            .ok()
            .map(|value| value.parse().map_err(|_| ConfigError::InvalidPort(value)))
            .transpose()?
            .unwrap_or(3000);
        let auth_token = optional_token("CLOUDLET_API_AUTH_TOKEN")?;
        let allow_remote = bool_env("CLOUDLET_API_ALLOW_REMOTE");

        if !is_loopback_host(&bind_host) {
            if !allow_remote {
                return Err(ConfigError::RemoteBindRequiresOptIn(bind_host));
            }
            if auth_token.is_none() {
                return Err(ConfigError::RemoteBindRequiresToken);
            }
        }

        let max_request_bytes = env::var("CLOUDLET_API_MAX_REQUEST_BYTES")
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| ConfigError::InvalidRequestLimit(value))
            })
            .transpose()?
            .unwrap_or(DEFAULT_MAX_REQUEST_BYTES);
        if max_request_bytes == 0 || max_request_bytes > DEFAULT_MAX_REQUEST_BYTES {
            return Err(ConfigError::RequestLimitOutOfRange(max_request_bytes));
        }

        Ok(Self {
            bind_host,
            bind_port,
            auth_token,
            max_request_bytes,
        })
    }

    pub fn requires_auth(&self) -> bool {
        self.auth_token.is_some()
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

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidPort(String),
    InvalidToken(&'static str),
    InvalidRequestLimit(String),
    RequestLimitOutOfRange(usize),
    RemoteBindRequiresOptIn(String),
    RemoteBindRequiresToken,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPort(value) => write!(formatter, "invalid CLOUDLET_API_PORT: {value}"),
            Self::InvalidToken(name) => write!(
                formatter,
                "{name} must contain at least 16 printable, non-space ASCII characters"
            ),
            Self::InvalidRequestLimit(value) => {
                write!(formatter, "invalid CLOUDLET_API_MAX_REQUEST_BYTES: {value}")
            }
            Self::RequestLimitOutOfRange(value) => write!(
                formatter,
                "CLOUDLET_API_MAX_REQUEST_BYTES must be between 1 and {DEFAULT_MAX_REQUEST_BYTES}, got {value}"
            ),
            Self::RemoteBindRequiresOptIn(host) => write!(
                formatter,
                "refusing non-loopback API bind {host}; set CLOUDLET_API_ALLOW_REMOTE=true and CLOUDLET_API_AUTH_TOKEN to opt in"
            ),
            Self::RemoteBindRequiresToken => write!(
                formatter,
                "a non-loopback API bind requires CLOUDLET_API_AUTH_TOKEN"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::is_loopback_host;

    #[test]
    fn recognises_only_explicit_loopback_hosts() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.10"));
    }
}
