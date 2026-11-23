//! `crown-context-inferd`: the model process of the visual context engine. It serves OCR,
//! embeddings and generation over crownos-ipc, picks models and devices through the
//! registry, takes images as sealed memfds, and has no network and no database.

mod admission;
mod convert;
mod dispatch;
mod engine;
mod job_gate;
mod logging;
mod sleep_guard;
mod util;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use clap::Parser;
use context_infer::Policy;
use crownos_ipc::ServiceBuilder;
use crownos_ipc::adapter::tokio::serve_with_outbox;
use tokio::signal::unix::{SignalKind, signal};

use crate::dispatch::{Dispatcher, RESIDENT, send_reply};
use crate::engine::Engine;
use crate::job_gate::JobGate;
use crate::util::memory::memory_tier;
use crate::util::power::on_battery;

/// How often the daemon reads the power state and checks for hung models.
const WATCH_INTERVAL: Duration = Duration::from_secs(5);

const DEFAULT_MODELS_DIR: &str = "/usr/share/crownos/context/models";

#[derive(Debug, Parser)]
#[command(about = "Runs the context engine's models")]
struct Cli {
    /// The model registry; defaults to /usr/share/crownos/context/registry.ron.
    #[arg(long)]
    registry: Option<PathBuf>,
    /// The directory the registry paths start from.
    #[arg(long, env = "CROWN_CONTEXT_MODELS", default_value = DEFAULT_MODELS_DIR)]
    models: PathBuf,
    /// The directory with the ggml device modules.
    #[arg(long)]
    ggml_modules: Option<PathBuf>,
    /// Directory for the IPC socket when systemd does not pass one; defaults to
    /// $XDG_RUNTIME_DIR/crownos.
    #[arg(long)]
    socket_dir: Option<PathBuf>,
    /// Seconds without use before a model that is not OCR or embedding unloads.
    #[arg(long, default_value_t = 120)]
    idle_unload_seconds: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    logging::init();
    let cli = Cli::parse();
    let engine = Arc::new(Engine::start(
        cli.registry.as_deref(),
        cli.models.clone(),
        cli.ggml_modules.as_deref(),
    )?);
    let mut builder = ServiceBuilder::new(context_protocol::INFERENCE_SERVICE);
    if let Some(directory) = &cli.socket_dir {
        builder = builder.directory(directory);
    }
    let mut server = match listenfd::ListenFd::from_env()
        .take_unix_listener(0)
        .context("reading the socket from systemd")?
    {
        Some(listener) => builder.build_with_listener(listener),
        None => builder.build(),
    }
    .context("binding the inference socket")?;
    tracing::info!(
        service = context_protocol::INFERENCE_SERVICE,
        "serving inference"
    );

    let (replies, mut outbox) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(JobGate::default());
    let dispatcher = Dispatcher::new(Arc::clone(&engine), replies, Arc::clone(&gate));
    tokio::spawn(async move {
        if let Err(error) = sleep_guard::guard_sleep(gate).await {
            tracing::warn!(%error, "jobs are not ended before sleep");
        }
    });
    let idle_unload = tokio::spawn(unload_idle_models(
        Arc::clone(&engine),
        Duration::from_secs(cli.idle_unload_seconds),
    ));
    let mut terminate = signal(SignalKind::terminate()).context("watching for SIGTERM")?;
    let watch = watch_power_and_health(Arc::clone(&engine));
    tokio::select! {
        served = serve_with_outbox(
            &mut server,
            |server, peer, message| dispatcher.dispatch(server, peer, message),
            |cx| outbox.poll_recv(cx),
            send_reply,
        ) => served.context("the inference service stopped")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
        _ = terminate.recv() => tracing::info!("stopping"),
        () = watch => {
            engine.save_benchmarks();
            tracing::error!("too many model threads hang; exiting so that systemd restarts the service");
            std::process::exit(1);
        }
    }
    idle_unload.abort();
    engine.save_benchmarks();
    Ok(())
}

/// Keeps the policy in step with the power state, and returns when hung model threads need
/// a restart of the process.
async fn watch_power_and_health(engine: Arc<Engine>) {
    let tier = memory_tier();
    let mut ticks = tokio::time::interval(WATCH_INTERVAL);
    loop {
        ticks.tick().await;
        let policy = Policy {
            on_battery: on_battery(),
            remote_consent: false,
            tier,
        };
        if engine.router.policy() != policy {
            tracing::info!(?policy, "the inference policy changed");
            engine.router.set_policy(policy);
        }
        if engine.router.needs_restart() {
            return;
        }
    }
}

/// Every quarter of `idle_for`, unloads what has not run for `idle_for` and saves the
/// latency cache.
async fn unload_idle_models(engine: Arc<Engine>, idle_for: Duration) {
    let mut ticks = tokio::time::interval((idle_for / 4).max(Duration::from_secs(1)));
    loop {
        ticks.tick().await;
        let engine = Arc::clone(&engine);
        let unloaded = tokio::task::spawn_blocking(move || {
            engine.save_benchmarks();
            engine.router.unload_idle(idle_for, &RESIDENT)
        })
        .await;
        if let Ok(count @ 1..) = unloaded {
            tracing::info!(count, "unloaded idle models");
        }
    }
}
