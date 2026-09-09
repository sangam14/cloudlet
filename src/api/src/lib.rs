pub mod config;
mod frontend;
pub mod models;
pub mod runtime;
pub mod service;
pub mod state;

use actix_web::{error::JsonPayloadError, http::StatusCode, web, App, HttpResponse, HttpServer};

use crate::{config::ApiConfig, state::AppState};

/// Runs the unprivileged HTTP control plane. This is used by both the
/// standalone `api` binary and the unified `cloudlet` executable. Both serve
/// the same embedded console at `/`.
pub async fn serve(config: ApiConfig) -> std::io::Result<()> {
    let bind_host = config.bind_host.clone();
    let bind_port = config.bind_port;
    let max_request_bytes = config.max_request_bytes;
    let state = web::Data::new(AppState::new(config));

    let server = HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(
                web::JsonConfig::default()
                    .limit(max_request_bytes)
                    .error_handler(|error, _request| {
                        let status = if matches!(error, JsonPayloadError::Overflow { .. } | JsonPayloadError::OverflowKnownLength { .. }) {
                            StatusCode::PAYLOAD_TOO_LARGE
                        } else {
                            StatusCode::BAD_REQUEST
                        };
                        actix_web::error::InternalError::from_response(
                            error,
                            HttpResponse::build(status)
                                .json(serde_json::json!({ "error": "request body is too large or invalid" })),
                        )
                        .into()
                    }),
            )
            .configure(service::configure)
            .configure(frontend::configure)
    })
        .bind((bind_host, bind_port))?;
    // Binding succeeds before advertising a URL, including when port 0 is used.
    for address in server.addrs() {
        println!("Cloudlet console: http://{address}/");
    }
    server.run().await
}

/// Keep the server on the calling thread so startup failures reach the caller.
pub fn serve_blocking(config: ApiConfig) -> std::io::Result<()> {
    actix_web::rt::System::new().block_on(serve(config))
}
