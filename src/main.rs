mod client;
mod config;
mod server;
mod middleware;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::Level::INFO.into())
                .from_env_lossy(),
        )
        .try_init()
        .expect("setting default subscriber failed");

    let config = match config::read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    let client = client::create_client(&config).await?;

    let (addr, handle) = server::start_server(&config, client).await?;

    tracing::info!("Server running at {addr}");

    handle.await?;

    Ok(())
}
