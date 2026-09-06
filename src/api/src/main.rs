use api::config::ApiConfig;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config = ApiConfig::try_from_env()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    println!(
        "Starting Cloudlet API on {}:{}",
        config.bind_host, config.bind_port
    );
    api::serve(config).await
}
