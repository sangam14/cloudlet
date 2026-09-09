//! Fixed-loopback llmman adapter. No arbitrary URLs, shell execution, provider
//! credentials, or guest networking cross this boundary.
use crate::{service::require_control_auth, state::AppState};
use actix_web::{web, HttpRequest, HttpResponse};
use futures::StreamExt;
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};

const BODY_LIMIT: usize = 1024 * 1024;

pub struct ModelRuntime {
    client: Result<Client, String>,
    endpoint: Result<String, String>,
    gate: Arc<Semaphore>,
    operation: Mutex<Option<ModelOperation>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelOperation {
    pub action: ModelAction,
    pub model: String,
    pub status: String,
    pub detail: String,
    pub output: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelAction {
    Pull,
    Run,
    Unload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequest {
    pub model: String,
    pub action: ModelAction,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Deserialize)]
struct ModelList {
    models: Vec<StoredModel>,
}

#[derive(Deserialize)]
struct StoredModel {
    name: String,
    #[serde(default)]
    size: u64,
}

#[derive(Serialize)]
pub struct ModelEntry {
    pub name: String,
    pub size_bytes: u64,
    pub loaded: bool,
    pub stored: bool,
}

#[derive(Serialize)]
pub struct ModelSnapshot {
    pub ready: bool,
    pub endpoint: String,
    pub detail: String,
    pub models: Vec<ModelEntry>,
    pub operation: Option<ModelOperation>,
}

impl ModelRuntime {
    pub fn from_env() -> Self {
        Self::new(
            &std::env::var("CLOUDLET_LLMMAN_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:17434".into()),
        )
    }

    fn new(endpoint: &str) -> Self {
        Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .build()
                .map_err(|_| "Unable to initialize model HTTP client".into()),
            endpoint: validate_endpoint(endpoint),
            gate: Arc::new(Semaphore::new(1)),
            operation: Mutex::new(None),
        }
    }

    async fn response(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        seconds: u64,
    ) -> Result<reqwest::Response, ModelFailure> {
        let endpoint = self.endpoint.as_ref().map_err(Clone::clone)?;
        let client = self.client.as_ref().map_err(Clone::clone)?;
        let mut request = client
            .request(method, format!("{endpoint}{path}"))
            // llmman's peer marker forces inventory/inference/unload to this
            // node only, even if the daemon has aggregation peers configured.
            .header("x-llmman-hop", "1")
            .timeout(Duration::from_secs(seconds));
        if let Some(body) = body {
            request = request.json(&body);
        }
        // Never forward browser headers: llmman interprets Authorization as
        // provider credentials, not as authentication for its local daemon.
        let response = request.send().await.map_err(|_| "llmman is unreachable or timed out. Start llmman serve on the configured loopback port.".to_string())?;
        if !response.status().is_success() {
            return Err(ModelFailure::confirmed(&format!(
                "llmman returned HTTP {}. Check its terminal for details.",
                response.status().as_u16()
            )));
        }
        Ok(response)
    }

    async fn json(&self, path: &str) -> Result<Value, String> {
        let response = self
            .response(Method::GET, path, None, 3)
            .await
            .map_err(|error| error.detail)?;
        serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| "Invalid JSON from llmman".into())
    }

    async fn inventory(&self) -> Result<Vec<ModelEntry>, String> {
        let (version, tags, ps) = tokio::try_join!(
            self.json("/api/version"),
            self.json("/api/tags"),
            self.json("/api/ps")
        )?;
        // llmman extends the compatibility endpoint with executable and PID.
        // Check the protocol, not merely whether something accepts TCP.
        if !version["version"].is_string()
            || !version["pid"].is_u64()
            || !(version["exe"].is_string() || version.get("exe").is_some_and(Value::is_null))
        {
            return Err("The endpoint does not identify itself as a llmman daemon".into());
        }
        let tags: ModelList =
            serde_json::from_value(tags).map_err(|_| "Invalid llmman model inventory")?;
        let ps: ModelList =
            serde_json::from_value(ps).map_err(|_| "Invalid llmman running inventory")?;
        let mut models: Vec<ModelEntry> = tags
            .models
            .into_iter()
            .map(|model| ModelEntry {
                loaded: ps.models.iter().any(|loaded| loaded.name == model.name),
                name: model.name,
                size_bytes: model.size,
                stored: true,
            })
            .collect();
        for model in ps.models {
            if !models.iter().any(|entry| entry.name == model.name) {
                models.push(ModelEntry {
                    name: model.name,
                    size_bytes: model.size,
                    loaded: true,
                    stored: false,
                });
            }
        }
        Ok(models)
    }

    pub async fn snapshot(&self) -> ModelSnapshot {
        let result = self.inventory().await;
        ModelSnapshot {
            ready: result.is_ok(),
            endpoint: self
                .endpoint
                .clone()
                .unwrap_or_else(|_| "Invalid configuration".into()),
            detail: result
                .as_ref()
                .map(|_| {
                    "llmman connected · host inference · guests remain network-isolated".into()
                })
                .unwrap_or_else(Clone::clone),
            models: result.unwrap_or_default(),
            operation: self.operation.lock().await.clone(),
        }
    }

    async fn start(self: &Arc<Self>, request: ModelRequest) -> Result<(), actix_web::Error> {
        validate_request(&request).map_err(actix_web::error::ErrorBadRequest)?;
        let permit = self.gate.clone().try_acquire_owned().map_err(|_| {
            actix_web::error::ErrorConflict("A model operation is already in progress")
        })?;
        if self
            .operation
            .lock()
            .await
            .as_ref()
            .is_some_and(|operation| operation.status == "unconfirmed")
        {
            return Err(actix_web::error::ErrorConflict("Previous upstream completion is unconfirmed. Check llmman, then restart Cloudlet after confirming it is idle."));
        }
        let inventory = self
            .inventory()
            .await
            .map_err(actix_web::error::ErrorServiceUnavailable)?;
        if request.action != ModelAction::Pull
            && !inventory.iter().any(|model| {
                model.name == request.model
                    && (request.action == ModelAction::Unload || model.stored)
            })
        {
            return Err(actix_web::error::ErrorBadRequest(
                "Select a downloaded model from the inventory first",
            ));
        }
        *self.operation.lock().await = Some(ModelOperation {
            action: request.action, model: request.model.clone(), status: "running".into(),
            detail: match request.action {
                ModelAction::Pull => "Downloading model. Progress depends on the registry; this can take several minutes.",
                ModelAction::Run => "Loading model and generating a reply. First use may download the inference engine.",
                ModelAction::Unload => "Unloading model from host memory.",
            }.into(), output: String::new(),
        });
        let runtime = self.clone();
        // Keep admission until upstream finishes even if this browser navigates
        // away. The last operation is observable by every authenticated client.
        tokio::spawn(async move {
            let result = runtime.perform(&request).await;
            if let Some(operation) = runtime.operation.lock().await.as_mut() {
                match result {
                    Ok(output) => {
                        operation.status = "done".into();
                        operation.detail = "Operation completed".into();
                        operation.output = output;
                    }
                    Err(error) => {
                        operation.status = if error.confirmed {
                            "failed"
                        } else {
                            "unconfirmed"
                        }
                        .into();
                        operation.detail = if error.confirmed {
                            error.detail
                        } else {
                            format!("{} Upstream work may still be active. Check llmman, then restart Cloudlet only after confirming it is idle.", error.detail)
                        };
                    }
                }
            }
            drop(permit);
        });
        Ok(())
    }

    async fn perform(&self, request: &ModelRequest) -> Result<String, ModelFailure> {
        if request.action == ModelAction::Pull {
            // llmman ignores stream:false for pulls and ALWAYS returns NDJSON.
            let response = self
                .response(
                    Method::POST,
                    "/api/pull",
                    Some(json!({"model":request.model})),
                    1800,
                )
                .await?;
            let mut stream = response.bytes_stream();
            let mut progress = PullProgress::default();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| "Download stream disconnected or timed out")?;
                if let Some(detail) = progress.push(&chunk)? {
                    if let Some(operation) = self.operation.lock().await.as_mut() {
                        operation.detail = detail;
                    }
                }
            }
            return progress.finish();
        }
        let body = if request.action == ModelAction::Unload {
            json!({"model":request.model,"keep_alive":0,"stream":false})
        } else {
            json!({"model":request.model,"prompt":request.prompt,"stream":false,"keep_alive":"5m","options":{"num_predict":256}})
        };
        let response = self
            .response(Method::POST, "/api/generate", Some(body), 600)
            .await?;
        let value: Value = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| "Invalid generation response from llmman")?;
        if value.get("error").is_some() {
            return Err(ModelFailure::confirmed(
                "llmman rejected the request. Check its terminal for details.",
            ));
        }
        if value["done"] != true {
            return Err("llmman did not confirm completion".into());
        }
        if request.action == ModelAction::Unload {
            if value["done_reason"] != "unload" {
                return Err("llmman did not acknowledge unloading".into());
            }
            return Ok("Unload acknowledged by llmman. Downloaded weights remain on disk.".into());
        }
        value["response"]
            .as_str()
            .filter(|reply| !reply.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ModelFailure::confirmed("The model completed without a text reply"))
    }
}

fn validate_endpoint(endpoint: &str) -> Result<String, String> {
    let invalid = || {
        "CLOUDLET_LLMMAN_URL must be an HTTP loopback IP and port, without credentials, path, or query".to_string()
    };
    let url = Url::parse(endpoint).map_err(|_| invalid())?;
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    if url.scheme() != "http"
        || !host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn validate_request(request: &ModelRequest) -> Result<(), String> {
    let model = &request.model;
    if model.is_empty()
        || model.len() > 256
        || !model.as_bytes()[0].is_ascii_alphanumeric()
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/@".contains(&b))
        || model.contains("://")
        || model
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Use a model name or OCI/Hugging Face reference (up to 256 ASCII characters), not a URL or file path".into());
    }
    if request.action == ModelAction::Run
        && (request.prompt.trim().is_empty() || request.prompt.len() > 8192)
    {
        return Err("Enter a prompt between 1 and 8192 bytes".into());
    }
    Ok(())
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "llmman connection interrupted or timed out; upstream work may still be running. Check llmman before retrying.")?;
        if body.len() + chunk.len() > BODY_LIMIT {
            return Err("llmman response exceeded the 1 MiB limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Debug)]
struct ModelFailure {
    detail: String,
    confirmed: bool,
}

impl ModelFailure {
    fn confirmed(detail: &str) -> Self {
        Self {
            detail: detail.into(),
            confirmed: true,
        }
    }
}
impl From<String> for ModelFailure {
    fn from(detail: String) -> Self {
        Self {
            detail,
            confirmed: false,
        }
    }
}
impl From<&str> for ModelFailure {
    fn from(detail: &str) -> Self {
        detail.to_string().into()
    }
}

#[derive(Default)]
struct PullProgress {
    pending: Vec<u8>,
    success: bool,
    notice: String,
}

impl PullProgress {
    fn frame(&mut self, line: &[u8]) -> Result<Option<String>, ModelFailure> {
        if line.is_empty() {
            return Ok(None);
        }
        let value: Value =
            serde_json::from_slice(line).map_err(|_| "Invalid download progress from llmman")?;
        if value.get("error").is_some() {
            return Err(ModelFailure::confirmed("Model download failed. Check the reference, available disk space, and llmman terminal."));
        }
        if let Some(notice) = value["notice"].as_str() {
            self.notice = notice.chars().take(4096).collect();
        }
        self.success = value["status"] == "success";
        Ok(value["status"]
            .as_str()
            .map(|status| status.chars().take(4096).collect()))
    }

    fn push(&mut self, bytes: &[u8]) -> Result<Option<String>, ModelFailure> {
        let mut detail = None;
        // Bound each frame, not a potentially 30-minute progress stream.
        for &byte in bytes {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.pending);
                if let Some(next) = self.frame(&line)? {
                    detail = Some(next);
                }
            } else {
                if self.pending.len() >= 16 * 1024 {
                    return Err("Download progress frame exceeded 16 KiB".into());
                }
                self.pending.push(byte);
            }
        }
        Ok(detail)
    }

    fn finish(mut self) -> Result<String, ModelFailure> {
        let line = std::mem::take(&mut self.pending);
        self.frame(&line)?;
        if self.success {
            Ok(format!(
                "Model downloaded. Select it to run a prompt.{}",
                if self.notice.is_empty() {
                    String::new()
                } else {
                    format!("\nVerification notice: {}", self.notice)
                }
            ))
        } else {
            Err("Download ended without confirmation".into())
        }
    }
}

pub fn configure(config: &mut web::ServiceConfig) {
    config
        .route("/api/v1/models", web::get().to(list))
        .route("/api/v1/models/operations", web::post().to(operation));
}

async fn list(
    request: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    require_control_auth(&request, &state.config)?;
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(state.models.snapshot().await))
}

async fn operation(
    http: HttpRequest,
    state: web::Data<AppState>,
    request: web::Json<ModelRequest>,
) -> Result<HttpResponse, actix_web::Error> {
    require_control_auth(&http, &state.config)?;
    state.models.start(request.into_inner()).await?;
    Ok(HttpResponse::Accepted().json(json!({"accepted":true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fixed_loopback_destinations() {
        for endpoint in ["http://127.0.0.1:17434", "http://[::1]:17434/"] {
            assert!(validate_endpoint(endpoint).is_ok());
        }
        for endpoint in [
            "http://localhost:17434",
            "https://127.0.0.1",
            "http://example.com",
            "http://127.0.0.1/api",
            "http://key@127.0.0.1",
            "http://127.0.0.1?x=1",
            "http://127.0.0.1#x",
        ] {
            assert!(validate_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn model_references_and_prompts_are_bounded() {
        for model in [
            "qwen3",
            "hf.co/unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M",
            "docker.io/ai/model:latest",
        ] {
            assert!(validate_request(&ModelRequest {
                model: model.into(),
                action: ModelAction::Run,
                prompt: "Hello".into()
            })
            .is_ok());
        }
        for model in [
            "../escape",
            "/tmp/model",
            "file:///tmp/model",
            "--help",
            "a\nb",
            "a/../b",
            "a?provider=x",
            "",
        ] {
            assert!(validate_request(&ModelRequest {
                model: model.into(),
                action: ModelAction::Pull,
                prompt: String::new()
            })
            .is_err());
        }
        assert!(validate_request(&ModelRequest {
            model: "m".into(),
            action: ModelAction::Run,
            prompt: "x".repeat(8193)
        })
        .is_err());
    }

    #[test]
    fn ndjson_download_requires_final_success_and_rejects_embedded_error() {
        let pull_result = |bytes: &[u8]| {
            let mut progress = PullProgress::default();
            progress.push(bytes)?;
            progress.finish()
        };
        assert!(
            pull_result(b"{\"status\":\"pulling manifest\"}\n{\"status\":\"success\"}\n").is_ok()
        );
        for body in [
            b"{\"status\":\"pulling manifest\"}\n".as_slice(),
            b"{\"status\":\"success\"}\n{\"error\":\"failed\"}\n",
            b"not json",
            b"",
        ] {
            assert!(pull_result(body).is_err());
        }
    }

    #[test]
    fn download_progress_is_incremental_and_bounded_per_frame() {
        let mut progress = PullProgress::default();
        for _ in 0..50000 {
            progress.push(b"{\"status\":\"downloading\"}\n").unwrap();
        }
        progress.push(b"{\"status\":\"suc").unwrap();
        progress
            .push(b"cess\",\"notice\":\"signature warning\"}\n")
            .unwrap();
        assert!(progress.finish().unwrap().contains("signature warning"));
        assert!(PullProgress::default().push(&vec![b'x'; 16385]).is_err());
    }

    // Protocol fixture only: never a demo server, model download, or inference.
    async fn fixture() -> (
        ModelRuntime,
        tokio::sync::mpsc::Receiver<String>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime = ModelRuntime::new(&format!("http://{}", listener.local_addr().unwrap()));
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 2048];
                loop {
                    let size = socket.read(&mut buffer).await.unwrap();
                    if size == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..size]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap_or("0")
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let body = match path {
                    "/api/version" => json!({"version":"fixture", "pid":1, "exe":null}).to_string(),
                    "/api/tags" => {
                        json!({"models":[{"name":"fixture-model","size":123}]}).to_string()
                    }
                    "/api/ps" => {
                        json!({"models":[{"name":"removed-from-store","size":456}]}).to_string()
                    }
                    "/api/pull" => {
                        "{\"status\":\"pulling manifest\"}\n{\"status\":\"success\"}\n".into()
                    }
                    "/api/generate" => {
                        let value: Value =
                            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap())
                                .unwrap();
                        if value["keep_alive"] == 0 {
                            json!({"done":true,"done_reason":"unload"}).to_string()
                        } else {
                            json!({"done":true,"response":"Fixture reply"}).to_string()
                        }
                    }
                    _ => panic!("Unexpected fixture path"),
                };
                sender.send(request).await.unwrap();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        (runtime, receiver, task)
    }

    #[tokio::test]
    async fn exercises_real_http_inventory_pull_run_and_unload_contracts() {
        let (runtime, mut received, fixture) = fixture().await;
        let inventory = runtime.inventory().await.unwrap();
        assert_eq!(inventory.len(), 2);
        assert!(inventory[0].stored && !inventory[0].loaded);
        assert!(!inventory[1].stored && inventory[1].loaded);
        for action in [ModelAction::Pull, ModelAction::Run, ModelAction::Unload] {
            let output = runtime
                .perform(&ModelRequest {
                    action,
                    model: "fixture-model".into(),
                    prompt: "Hello".into(),
                })
                .await
                .unwrap();
            assert!(!output.is_empty());
        }
        for _ in 0..6 {
            let request = received.recv().await.unwrap();
            assert!(request.to_ascii_lowercase().contains("x-llmman-hop: 1"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            if request.starts_with("POST /api/generate") {
                let value: Value =
                    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                assert_eq!(value["stream"], false);
                if value["prompt"] == "Hello" {
                    assert_eq!(value["options"]["num_predict"], 256);
                }
            }
        }
        fixture.abort();
    }

    #[tokio::test]
    async fn rejects_missing_model_busy_and_unconfirmed_operations() {
        let (runtime, _received, fixture) = fixture().await;
        let runtime = Arc::new(runtime);
        let request = || ModelRequest {
            action: ModelAction::Run,
            model: "not-stored".into(),
            prompt: "Hello".into(),
        };
        assert_eq!(
            runtime
                .start(request())
                .await
                .unwrap_err()
                .as_response_error()
                .status_code(),
            actix_web::http::StatusCode::BAD_REQUEST
        );
        let permit = runtime.gate.clone().acquire_owned().await.unwrap();
        assert_eq!(
            runtime
                .start(request())
                .await
                .unwrap_err()
                .as_response_error()
                .status_code(),
            actix_web::http::StatusCode::CONFLICT
        );
        drop(permit);
        *runtime.operation.lock().await = Some(ModelOperation {
            action: ModelAction::Pull,
            model: "m".into(),
            status: "unconfirmed".into(),
            detail: String::new(),
            output: String::new(),
        });
        assert_eq!(
            runtime
                .start(request())
                .await
                .unwrap_err()
                .as_response_error()
                .status_code(),
            actix_web::http::StatusCode::CONFLICT
        );
        fixture.abort();
    }

    #[tokio::test]
    async fn definitive_http_rejection_does_not_require_operator_reconciliation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime = ModelRuntime::new(&format!("http://{}", listener.local_addr().unwrap()));
        let fixture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 2048];
            let mut received = 0;
            while !bytes[..received].windows(4).any(|part| part == b"\r\n\r\n") {
                assert!(received < bytes.len(), "fixture request headers too large");
                let count = socket.read(&mut bytes[received..]).await.unwrap();
                assert!(count > 0, "fixture request ended before headers completed");
                received += count;
            }
            socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let failure = runtime
            .response(Method::GET, "/api/version", None, 3)
            .await
            .unwrap_err();
        assert!(failure.confirmed);
        assert!(failure.detail.contains("503"));
        fixture.await.unwrap();
    }
}
