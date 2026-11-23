//! `crown-contextd`: observes through the compositor bridge (or a replayed trace), passes every
//! observation through the policy, lets the engine segment it, persists what it should, and
//! answers the Context API on crownos-ipc.

mod access;
mod accessibility_host;
mod api;
mod capture;
mod capture_output;
mod inferd_connection;
mod local_sensitivity;
mod logging;
mod pipeline;
#[cfg(test)]
mod pipeline_privacy_tests;
mod shared;
mod sleep_guard;
mod structured_text;
mod text_gate;
mod util;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::Parser;
use context_collect::a11y::{AccessibilityBus, AccessibilityCollector, RecentReadings};
use context_collect::{CompositorBridge, CompositorLink, replay};
use context_eval::Trace;
use context_policy::{Policy, VisualContext};
use context_store::{Store, Writer};
use crownos_config::schema::{ContextAccess, ContextDecision};
use crownos_ipc::adapter::tokio::serve_with_outbox;
use crownos_ipc::{Caller, Error as IpcError, ServiceBuilder};

use crate::access::{Grants, Parked, answer_parked, park_or_refuse};
use crate::accessibility_host::ContextHost;
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
    let cli = Cli::parse();
    let mut section: VisualContext = crownos_config::load(VisualContext::SECTION);
    if cli.replay.is_some() {
        section.enabled = true;
    }
    let link = if cli.replay.is_some() {
        CompositorLink::with_engine_scope()
    } else {
        CompositorLink::default()
    };
    let observes_the_compositor = cli.replay.is_none() && section.enabled;
    // inferd reads this program from /proc when it accepts, so connect while that works.
    let inferd = observes_the_compositor
        .then(|| inferd_connection::connect(link.clone(), cli.socket_dir.clone()));
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .context("turning off core dumps and ptrace for this process")?;
    let policy = Arc::new(Policy::from_section(&section).context("reading visual_context.ron")?);
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
    let mut server = match builder.verify_sender_per_frame(true).build() {
        Err(IpcError::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported => {
            tracing::warn!(%error, "the kernel cannot check each sender; serving without it");
            let mut builder = ServiceBuilder::new(context_protocol::SERVICE);
            if let Some(dir) = &cli.socket_dir {
                builder = builder.directory(dir);
            }
            builder.build().context("binding the context socket")?
        }
        built => built.context("binding the context socket")?,
    };

    let (outlet, observations) = mpsc::sync_channel(OBSERVATION_QUEUE);
    let shared = Arc::new(Mutex::new(Shared::new(section.enabled)));
    let readings = RecentReadings::default();
    let accessibility = if observes_the_compositor {
        let host = ContextHost::new(link.clone(), Arc::clone(&policy), Arc::clone(&shared));
        Some(
            AccessibilityCollector::spawn(
                outlet.clone(),
                host,
                AccessibilityBus::Session,
                readings.clone(),
            )
            .context("starting the accessibility collector")?,
        )
    } else {
        None
    };
    // The capture pipeline sets the exposure hook before the bridge first connects.
    let capturing = if let Some(inferd) = inferd {
        let service = capture::spawn(&link, outlet.clone(), inferd, readings)?;
        tokio::spawn({
            let service = Arc::clone(&service);
            async move {
                if let Err(error) = sleep_guard::guard_sleep(service).await {
                    tracing::warn!(%error, "frames are not zeroed before sleep");
                }
            }
        });
        Some(service)
    } else {
        None
    };
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
        None => Some(
            CompositorBridge::spawn(outlet, link.clone())
                .context("starting the compositor bridge")?,
        ),
    };

    let (events, outbox) = tokio::sync::mpsc::unbounded_channel();
    let grants = Grants::load();
    let _grants_watch = {
        let grants = grants.clone();
        let events = events.clone();
        crownos_config::subscribe_typed::<ContextAccess, _>(ContextAccess::SECTION, move |access| {
            grants.replace(access);
            let _ = events.send(pipeline::Outgoing::GrantsChanged);
        })
    };
    let questions = events.clone();
    let pipeline = pipeline::spawn(
        observations,
        policy,
        writer.handle(),
        Arc::clone(&shared),
        events,
        link,
        capturing,
    )?;

    tracing::info!(
        service = context_protocol::SERVICE,
        "serving the Context API"
    );
    let handler = RefCell::new(api::Api::new(
        Arc::clone(&shared),
        reader,
        writer.handle(),
        bridge,
        grants.clone(),
    ));
    let parked = RefCell::new(Parked::default());
    let mut outbox = outbox;
    tokio::select! {
        served = serve_with_outbox(
            &mut server,
            |server, peer, message| {
                let key = server
                    .credentials(peer)
                    .and_then(|credentials| grants.key_of(Caller { peer, credentials }));
                if let Some(key) = key.filter(|key| grants.decide(Some(key)) == ContextDecision::Ask) {
                    park_or_refuse(server, &mut parked.borrow_mut(), key, peer, message, &questions);
                    return Ok(true);
                }
                context_protocol::context::dispatch_with_peer(&mut *handler.borrow_mut(), server, peer, message).map(|()| true)
            },
            |cx| outbox.poll_recv(cx),
            |server, outgoing| match outgoing {
                pipeline::Outgoing::Answered { key, answer } => answer_parked(
                    server,
                    &grants,
                    &mut handler.borrow_mut(),
                    &mut parked.borrow_mut(),
                    &key,
                    answer,
                ),
                other => api::send_event(server, &grants, other),
            },
        ) => served.context("the Context API stopped")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
    }
    drop(accessibility);
    drop(handler);
    pipeline.stop();
    drop(writer);
    Ok(())
}
