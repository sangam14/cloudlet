use crate::{
    client::vmmorchestrator::{execute_response::Stage, ExecuteResponse, ShutdownVmResponse},
    state::AppState,
};
use actix_web::{http::header::AUTHORIZATION, web, HttpRequest, HttpResponse, Responder};
use actix_web_lab::sse;
use async_stream::stream;
use control_plane::{
    CapacitySummary, ConnectionSummary, DashboardSnapshot, HealthResponse, ServiceStatus,
    ShutdownResponse, WorkloadRequest,
};
use serde::Serialize;
use tokio_stream::StreamExt;

const MAX_WORKLOAD_NAME_BYTES: usize = 64;
const MAX_CODE_BYTES: usize = 240 * 1024;

/// Public control-plane HTTP boundary. `/run` and `/shutdown` are retained as
/// compatibility aliases while callers migrate to versioned endpoints.
pub fn configure(config: &mut web::ServiceConfig) {
    config
        .route("/healthz", web::get().to(health))
        .route("/api/v1/overview", web::get().to(overview))
        .route("/api/v1/workloads", web::post().to(run))
        .route("/api/v1/sandboxes/{id}/stop", web::post().to(stop_sandbox))
        .route("/api/v1/sandboxes/{id}", web::delete().to(remove_sandbox))
        .route("/api/v1/vmm/shutdown", web::post().to(shutdown))
        .route("/run", web::post().to(run))
        .route("/shutdown", web::post().to(shutdown));
}

pub async fn health(state: web::Data<AppState>) -> impl Responder {
    let status = runtime_status(&state);
    HttpResponse::Ok().json(HealthResponse {
        service: "cloudlet-api".into(),
        status,
        vmm_endpoint: "embedded://boxlite".into(),
        detail: if state.runtime.ready() {
            "BoxLite initialized".into()
        } else {
            "BoxLite unavailable; see authenticated overview for setup details".into()
        },
    })
}

/// Persistent BoxLite inventory, with readiness separate from HTTP connectivity.
pub async fn overview(
    request: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    require_control_auth(&request, &state.config)?;
    let mut status = runtime_status(&state);
    let mut detail = state.runtime.detail();
    let (capacity, workloads) = match state.runtime.inventory().await {
        Ok(inventory) => inventory,
        Err(error) => {
            status = ServiceStatus::Degraded;
            detail = error;
            (
                CapacitySummary {
                    running: 0,
                    stopped: 0,
                    vcpus: 0,
                    memory_mib: 0,
                },
                Vec::new(),
            )
        }
    };

    Ok(HttpResponse::Ok().json(DashboardSnapshot {
        connection: ConnectionSummary {
            label: "BoxLite runtime".into(),
            endpoint: "embedded://boxlite".into(),
            status,
            detail,
        },
        capacity,
        workloads,
        // The old llmman bridge belongs to the legacy TAP runtime. Do not
        // advertise it as connected to these network-disabled sandboxes.
        models: Vec::new(),
    }))
}

pub async fn run(
    http_request: HttpRequest,
    state: web::Data<AppState>,
    request: web::Json<WorkloadRequest>,
) -> Result<impl Responder, actix_web::Error> {
    require_control_auth(&http_request, &state.config)?;
    let request = request.into_inner();
    validate_workload_request(&request)?;

    if !state.runtime.ready() {
        return Err(actix_web::error::ErrorServiceUnavailable(
            state.runtime.detail(),
        ));
    }
    let permit = state
        .runtime
        .reserve()
        .map_err(actix_web::error::ErrorConflict)?;
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    let runtime = state.runtime.clone();
    tokio::spawn(runtime.execute(request, sender, permit));
    let mut response_stream = tokio_stream::wrappers::ReceiverStream::new(receiver);

    let stream = stream! {
        while let Some(response) = response_stream.next().await {
            match sse::Data::new_json(response) {
                    Ok(data) => yield sse::Event::Data(data),
                    Err(error) => yield sse::Event::Comment(format!("serialisation error: {error}").into()),
            }
        }
    };

    Ok(
        sse::Sse::from_infallible_stream(stream)
            .with_keep_alive(std::time::Duration::from_secs(10)),
    )
}

pub async fn shutdown(
    request: HttpRequest,
    state: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    require_control_auth(&request, &state.config)?;

    Ok(HttpResponse::Ok().json(ShutdownResponse {
        success: state.runtime.cancel_active().await,
    }))
}

async fn stop_sandbox(
    request: HttpRequest,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
    require_control_auth(&request, &state.config)?;
    state
        .runtime
        .stop(&id)
        .await
        .map_err(actix_web::error::ErrorConflict)?;
    Ok(HttpResponse::Ok().json(ShutdownResponse { success: true }))
}

async fn remove_sandbox(
    request: HttpRequest,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
    require_control_auth(&request, &state.config)?;
    state
        .runtime
        .remove(&id)
        .await
        .map_err(actix_web::error::ErrorConflict)?;
    Ok(HttpResponse::Ok().json(ShutdownResponse { success: true }))
}

fn runtime_status(state: &AppState) -> ServiceStatus {
    if state.runtime.ready() {
        ServiceStatus::Ready
    } else {
        ServiceStatus::Degraded
    }
}

fn require_control_auth(
    request: &HttpRequest,
    config: &crate::config::ApiConfig,
) -> Result<(), actix_web::Error> {
    require_browser_origin(request, config)?;
    let Some(expected_token) = config.auth_token.as_deref() else {
        return Ok(());
    };

    let supplied = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "));

    if supplied == Some(expected_token) {
        Ok(())
    } else {
        Err(actix_web::error::ErrorUnauthorized(
            "missing or invalid Cloudlet API bearer token",
        ))
    }
}

/// A loopback API is still reachable by browsers visiting other websites.
/// Reject cross-origin requests and foreign Host names before privileged work.
/// Non-browser clients without Origin remain supported with the same token policy.
fn require_browser_origin(
    request: &HttpRequest,
    config: &crate::config::ApiConfig,
) -> Result<(), actix_web::Error> {
    let host = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .or_else(|| request.uri().authority().map(|value| value.as_str()))
        .ok_or_else(|| actix_web::error::ErrorForbidden("missing Host header"))?;
    let uri = format!("http://{host}")
        .parse::<actix_web::http::Uri>()
        .map_err(|_| actix_web::error::ErrorForbidden("invalid Host header"))?;
    let host_name = uri.host().unwrap_or("").trim_matches(['[', ']']);
    let is_loopback = |name: &str| {
        name.eq_ignore_ascii_case("localhost")
            || name
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    };
    if is_loopback(&config.bind_host) && !is_loopback(host_name) {
        return Err(actix_web::error::ErrorForbidden(
            "foreign Host rejected by local API",
        ));
    }
    if let Some(origin) = request.headers().get("origin") {
        let origin = origin
            .to_str()
            .ok()
            .and_then(|value| value.parse::<actix_web::http::Uri>().ok());
        let same_origin = origin.as_ref().is_some_and(|origin| {
            matches!(origin.scheme_str(), Some("http" | "https"))
                && origin
                    .authority()
                    .is_some_and(|authority| authority.as_str().eq_ignore_ascii_case(host))
        });
        if !same_origin {
            return Err(actix_web::error::ErrorForbidden(
                "cross-origin control request rejected",
            ));
        }
    }
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|value| value == "cross-site")
    {
        return Err(actix_web::error::ErrorForbidden(
            "cross-site control request rejected",
        ));
    }
    Ok(())
}

fn validate_workload_request(request: &WorkloadRequest) -> Result<(), actix_web::Error> {
    let valid_name = !request.workload_name.is_empty()
        && request.workload_name.len() <= MAX_WORKLOAD_NAME_BYTES
        && request
            .workload_name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid_name {
        return Err(actix_web::error::ErrorBadRequest(
            "workload_name must be 1-64 lowercase ASCII letters, digits, or hyphens",
        ));
    }

    if request.code.trim().is_empty() || request.code.len() > MAX_CODE_BYTES {
        return Err(actix_web::error::ErrorPayloadTooLarge(format!(
            "code must be between 1 and {MAX_CODE_BYTES} bytes"
        )));
    }

    if request.action != "prepare-and-run" {
        return Err(actix_web::error::ErrorBadRequest(
            "only action=prepare-and-run is currently supported",
        ));
    }

    if request.server.address.len() > 253 {
        return Err(actix_web::error::ErrorBadRequest(
            "server.address is too long",
        ));
    }

    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ExecuteJsonResponse {
    pub stage: StageJson,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub raw_output: bool,
}

#[derive(Debug, Serialize)]
pub enum StageJson {
    Pending,
    Building,
    Running,
    Done,
    Failed,
    Debug,
}

impl From<Stage> for StageJson {
    fn from(value: Stage) -> Self {
        match value {
            Stage::Pending => Self::Pending,
            Stage::Building => Self::Building,
            Stage::Running => Self::Running,
            Stage::Done => Self::Done,
            Stage::Failed => Self::Failed,
            Stage::Debug => Self::Debug,
        }
    }
}

impl From<ExecuteResponse> for ExecuteJsonResponse {
    fn from(value: ExecuteResponse) -> Self {
        Self {
            stage: Stage::try_from(value.stage).unwrap_or(Stage::Failed).into(),
            stdout: value.stdout,
            stderr: value.stderr,
            exit_code: value.exit_code,
            raw_output: false,
        }
    }
}

impl From<ShutdownVmResponse> for ShutdownResponse {
    fn from(value: ShutdownVmResponse) -> Self {
        Self {
            success: value.success,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{require_control_auth, validate_workload_request};
    use crate::config::ApiConfig;
    use actix_web::test::TestRequest;
    use control_plane::{BuildConfig, Language, LogLevel, ServerConfig, WorkloadRequest};
    use std::path::PathBuf;

    fn request(name: &str) -> WorkloadRequest {
        WorkloadRequest {
            workload_name: name.into(),
            language: Language::Rust,
            code: "fn main() {}".into(),
            log_level: LogLevel::Info,
            action: "prepare-and-run".into(),
            server: ServerConfig {
                address: "localhost".into(),
                port: 50051,
            },
            build: BuildConfig {
                source_code_path: PathBuf::from("/unused-by-api"),
                release: true,
            },
        }
    }

    #[test]
    fn rejects_path_like_workload_names() {
        assert!(validate_workload_request(&request("../escape")).is_err());
    }

    #[test]
    fn accepts_a_valid_request_shape() {
        assert!(validate_workload_request(&request("safe-name")).is_ok());
    }

    fn local_config(token: Option<&str>) -> ApiConfig {
        ApiConfig {
            bind_host: "127.0.0.1".into(),
            bind_port: 3000,
            vmm_endpoint: "http://[::1]:50051".into(),
            auth_token: token.map(str::to_string),
            vmm_auth_token: None,
            max_request_bytes: 256 * 1024,
        }
    }

    #[test]
    fn browser_control_requires_local_host_and_same_origin() {
        let config = local_config(None);
        let same_origin = TestRequest::default()
            .insert_header(("host", "127.0.0.1:3000"))
            .insert_header(("origin", "http://127.0.0.1:3000"))
            .to_http_request();
        assert!(require_control_auth(&same_origin, &config).is_ok());
        for origin in ["https://example.com", "null", "http://127.0.0.1:4000"] {
            let request = TestRequest::default()
                .insert_header(("host", "127.0.0.1:3000"))
                .insert_header(("origin", origin))
                .to_http_request();
            assert!(require_control_auth(&request, &config).is_err());
        }
        let rebinding = TestRequest::default()
            .insert_header(("host", "example.com:3000"))
            .insert_header(("origin", "http://example.com:3000"))
            .to_http_request();
        assert!(require_control_auth(&rebinding, &config).is_err());
        let cross_site = TestRequest::default()
            .insert_header(("host", "localhost:3000"))
            .insert_header(("sec-fetch-site", "cross-site"))
            .to_http_request();
        assert!(require_control_auth(&cross_site, &config).is_err());
    }

    #[test]
    fn native_clients_and_ipv6_obey_the_same_bearer_policy() {
        let config = local_config(Some("test-token-123456789"));
        let request = TestRequest::default()
            .insert_header(("host", "[::1]:3000"))
            .to_http_request();
        assert!(require_control_auth(&request, &config).is_err());
        let request = TestRequest::default()
            .insert_header(("host", "[::1]:3000"))
            .insert_header(("authorization", "Bearer test-token-123456789"))
            .to_http_request();
        assert!(require_control_auth(&request, &config).is_ok());
    }
}
