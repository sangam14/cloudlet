use crate::utils::ConfigFileHandler;
use control_plane::{BuildConfig, Language, ServerConfig, ShutdownResponse, WorkloadRequest};
use reqwest::Client;
use serde::Deserialize;
use std::{env, error::Error};

#[derive(Deserialize, Debug)]
struct TomlConfig {
    #[serde(rename = "workload-name")]
    workload_name: String,
    language: Language,
    action: String,
    server: ServerConfig,
    build: BuildConfig,
}

pub struct CloudletClient {}

impl CloudletClient {
    pub fn new_cloudlet_config(config: String) -> WorkloadRequest {
        let config: TomlConfig =
            toml::from_str(&config).expect("Error while parsing the config file");

        let workload_name = config.workload_name;
        let code: String = ConfigFileHandler::read_file(&config.build.source_code_path)
            .expect("Error while reading the code file");

        let language = config.language;
        WorkloadRequest {
            workload_name,
            language,
            code,
            log_level: control_plane::LogLevel::Info,
            server: config.server,
            build: config.build,
            action: config.action,
        }
    }

    pub async fn run(request: WorkloadRequest) -> Result<(), Box<dyn Error>> {
        let client = Client::new();
        let json = serde_json::to_string(&request)?;
        let endpoint = format!("{}/api/v1/workloads", Self::api_endpoint());
        let res = Self::with_api_auth(
            client
                .post(endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(json),
        )
        .send()
        .await?;

        println!("Response: {:?}", res.text().await?);
        Ok(())
    }

    pub async fn shutdown() -> Result<bool, ()> {
        let client = Client::new();
        let endpoint = format!("{}/api/v1/workloads/cancel", Self::api_endpoint());
        let response = Self::with_api_auth(client.post(endpoint)).send().await;

        let shutdown_response: ShutdownResponse =
            response.unwrap().json::<ShutdownResponse>().await.unwrap();

        Ok(shutdown_response.success)
    }

    fn api_endpoint() -> String {
        env::var("CLOUDLET_API_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:3000".into())
            .trim_end_matches('/')
            .to_owned()
    }

    fn with_api_auth(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match env::var("CLOUDLET_API_AUTH_TOKEN") {
            Ok(token) if !token.is_empty() => {
                request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            }
            _ => request,
        }
    }
}
