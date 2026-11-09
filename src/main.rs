mod article;
mod config;
mod db;
mod llm;
mod pipeline;
mod render;
mod sources;
mod story;
mod telegram;

#[cfg(test)]
mod tests;

use clap::Parser;
use config::{Cli, Command};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (cfg, forever) = match cli.command {
        Command::Run(c) => (c, true),
        Command::Once(c) => (c, false),
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if cfg.log_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    cfg.validate()?;
    let interval = Duration::from_secs(cfg.poll_interval_seconds);
    let app = pipeline::App::new(cfg).await?;

    loop {
        match app.cycle().await {
            Ok(s) => tracing::info!(
                seen = s.seen,
                new = s.new,
                posted = s.posted,
                edited = s.edited,
                failed = s.failed,
                "cycle done"
            ),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "cycle failed"),
        }
        if !forever {
            return Ok(());
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
