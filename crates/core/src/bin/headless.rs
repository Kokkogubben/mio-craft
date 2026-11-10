//! Runs the launcher without a window: just the local API.
//! Handy for development (`cargo run -p launcher-core --bin launcher-headless`)
//! and for testing the MCP server.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = launcher_core::Paths::new(launcher_core::Paths::default_root());
    let l = launcher_core::Launcher::new(paths).await?;
    let info = launcher_core::api::serve(l.clone()).await?;
    println!("MioCraft API on http://127.0.0.1:{} (data: {})", info.port, l.paths.root.display());
    println!("Token written to {}", l.paths.api_file().display());
    tokio::signal::ctrl_c().await?;
    let _ = tokio::fs::remove_file(l.paths.api_file()).await;
    Ok(())
}
