//! Runs the standalone development host; embedding remains a library concern.
use devermind_server::{Config, DevServer};

/// Keeps the server handle alive until Ctrl-C, then drops its watcher and listener.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let server = DevServer::start(config)?;
    println!("devermind ready at {}", server.url());
    tokio::signal::ctrl_c().await?;
    server.stop();
    Ok(())
}
