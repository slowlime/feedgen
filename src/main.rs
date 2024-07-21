mod cli;
mod config;
mod extractor;
mod fetch;
mod server;
mod state;
mod storage;
mod template;
mod xpath;

use std::process::ExitCode;

use anyhow::Result;
use cli::Args;
use fetch::Fetcher;
use server::Server;
use state::State;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::Level;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

fn set_up_logging() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            EnvFilter::builder()
                .with_regex(false)
                .with_default_directive(Level::INFO.into())
                .with_env_var("FEEDGEN_LOG")
                .from_env_lossy(),
        )
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    set_up_logging();

    let cancel = CancellationToken::new();

    tokio::spawn({
        let cancel = cancel.clone();

        async move {
            tokio::signal::ctrl_c().await.unwrap();
            cancel.cancel();
        }
    });

    let mut tasks = match start(cancel.clone()).await {
        Ok(tasks) => tasks,

        Err(e) => {
            error!("{e:#}");
            return ExitCode::FAILURE;
        }
    };

    let mut exit_code = ExitCode::SUCCESS;

    while let Some(task_result) = tasks.join_next().await {
        cancel.cancel();

        if let Err(e) = task_result {
            error!("{e:#}");
            exit_code = ExitCode::FAILURE;
        }
    }

    exit_code
}

async fn start(cancel: CancellationToken) -> Result<JoinSet<Result<()>>> {
    let mut args = Args::parse();
    let config_paths = args
        .config_path
        .take()
        .into_iter()
        .chain(["./feedgen.toml".into(), "/etc/feedgen.toml".into()])
        .collect::<Vec<_>>();
    let mut config = config::load(&config_paths)?;
    config.update(args);
    let state = State::new(config).await?;

    let fetcher = Fetcher::new(
        state.feeds.clone(),
        state.cfg.cache_dir.clone(),
        state.storage.clone(),
        state.cfg.max_initial_fetch_sleep.into(),
    );
    let server = Server::new(state).await?;

    let mut tasks = JoinSet::new();
    tasks.spawn(fetcher.run(cancel.clone()));
    tasks.spawn(server.serve(cancel.clone()));

    Ok(tasks)
}
