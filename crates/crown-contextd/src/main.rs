//! `crown-contextd`: observes through the compositor bridge (or a replayed trace), passes every
//! observation through the policy, lets the engine segment it, persists what it should, and
//! answers the Context API on crownos-ipc.

mod api;
mod capture;
mod capture_output;
mod local_sensitivity;
mod logging;
mod pipeline;
#[cfg(test)]
mod pipeline_privacy_tests;
mod shared;
mod sleep_guard;
mod util;

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use context_collect::{CompositorBridge, replay};
use context_eval::Trace;
use context_policy::{Policy, VisualContext};
use context_store::{Store, Writer};
use crownos_ipc::ServiceBuilder;
use crownos_ipc::adapter::tokio::serve_with_outbox;

use crate::shared::Shared;
use crate::util::paths::state_dir;

const OBSERVATION_QUEUE: usize = 4_096;

#[derive(Debug, Parser)]
#[command(name = "crown-contextd", about, version)]
struct Cli {
    /// Replay a trace from context-eval instead of binding the compositor.
    #[arg(long)]
    replay: Option<PathBuf>,
    /// How many times faster than real time a replay runs; 0 is as fast as possible.
    #[arg(long, default_value_t = 0.0)]
    speed: f64,
    /// Where the database and key live; defaults to $XDG_STATE_HOME/crownos/context.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Directory for the IPC socket; defaults to $XDG_RUNTIME_DIR/crownos.
    #[arg(long)]
    socket_dir: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    logging::init();
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .context("turning off core dumps and ptrace for this process")?;
    let cli = Cli::parse();
    let mut section: VisualContext = crownos_config::load(VisualContext::SECTION);
    if cli.replay.is_some() {
        section.enabled = true;
    }
    let policy = Policy::from_section(&section).context("reading visual_context.ron")?;
    let state_dir = cli.state_dir.clone().unwrap_or_else(state_dir);
    let writer = Writer::spawn(Store::open_dir(&state_dir).context("opening the store")?)
        .context("starting the writer")?;
    let reader = Store::open_dir(&state_dir).context("opening the store for reads")?;
    tracing::info!(state_dir = %state_dir.display(), encrypted = Store::is_encrypted(), "store open");

    // Bind the socket before a thread starts, so a second instance stops before it observes.
    let mut builder = ServiceBuilder::new(context_protocol::SERVICE);
    if let Some(dir) = &cli.socket_dir {
        builder = builder.directory(dir);
    }
    let mut server = builder.build().context("binding the context socket")?;

    let (outlet, observations) = mpsc::sync_channel(OBSERVATION_QUEUE);
    let capture_outlet = outlet.clone();
    let shared = Arc::new(Mutex::new(Shared::new(section.enabled)));
    let bridge = match &cli.replay {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let trace = Trace::from_json(&text).context("parsing the trace")?;
            tracing::info!(observations = trace.observations.len(), "replaying a trace");
            let speed = cli.speed;
            std::thread::Builder::new()
                .name("context-replay".to_owned())
                .spawn(move || replay(trace.observations, &outlet, speed))?;
            None
        }
        None => Some(CompositorBridge::spawn(outlet).context("starting the compositor bridge")?),
    };

    let capturing = match &bridge {
        Some(bridge) if section.enabled => {
            let link = bridge.link();
            let service = capture::spawn(&link, capture_outlet, cli.socket_dir.clone())?;
            tokio::spawn({
                let service = Arc::clone(&service);
                async move {
                    if let Err(error) = sleep_guard::guard_sleep(service).await {
                        tracing::warn!(%error, "frames are not zeroed before sleep");
                    }
                }
            });
            Some(pipeline::Capturing { service, link })
        }
        _ => None,
    };

    let (events, outbox) = tokio::sync::mpsc::unbounded_channel();
    let pipeline = pipeline::spawn(
        observations,
        policy,
        writer.handle(),
        Arc::clone(&shared),
        events,
        capturing,
    )?;

    tracing::info!(
        service = context_protocol::SERVICE,
        "serving the Context API"
    );
    let mut handler = api::Api::new(Arc::clone(&shared), reader, writer.handle(), bridge);
    let mut outbox = outbox;
    tokio::select! {
        served = serve_with_outbox(
            &mut server,
            |server, peer, message| context_protocol::context::dispatch(&mut handler, server, peer, message).map(|()| true),
            |cx| outbox.poll_recv(cx),
            api::send_event,
        ) => served.context("the Context API stopped")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
    }
    drop(handler);
    pipeline.stop();
    drop(writer);
    Ok(())
}
