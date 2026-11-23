//! `a11y-cost`: runs the accessibility collector on a private a11y bus for the cost measurement.
//! `listen` registers the events and never reads; `read` also reads the one window it is told
//! about, as if the compositor had focused it. Each observation prints one line.

use std::sync::mpsc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use context_collect::WindowTable;
use context_collect::a11y::{
    AccessibilityBus, AccessibilityCollector, AccessibilityHost, FrameContent, RecentReadings,
    read_active_window,
};
use context_core::{ObservationKind, WindowIdent, WindowRecord};
use tokio::sync::watch;

#[derive(Debug, Parser)]
#[command(name = "a11y-cost", about)]
struct Cli {
    /// The D-Bus address of the private a11y bus.
    #[arg(long)]
    bus: String,
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Debug, Subcommand)]
enum Mode {
    /// Register the events only: no window is known, so nothing is read.
    Listen,
    /// Read the window of `pid` whenever the collector would.
    Read {
        #[arg(long)]
        pid: i32,
    },
    /// Read the active window of the application `bus_name` once per budget, and print what
    /// each read found and how long it took.
    Probe {
        #[arg(long)]
        bus_name: String,
        #[arg(long, value_delimiter = ',', default_value = "200,500,1000,2000,5000")]
        budgets_ms: Vec<u64>,
    },
}

/// A compositor that knows at most one window, focused, that may always be read.
#[derive(Debug)]
struct OneWindowHost {
    windows: WindowTable,
    focus: watch::Sender<Option<WindowIdent>>,
}

impl OneWindowHost {
    fn new(pid: Option<i32>) -> Self {
        let mut windows = WindowTable::default();
        let focused = pid.map(|pid| {
            let ident = WindowIdent("measured".into());
            windows.upsert(WindowRecord {
                ident: ident.clone(),
                pid,
                ..WindowRecord::default()
            });
            ident
        });
        windows.set_focus(focused.clone());
        Self {
            windows,
            focus: watch::Sender::new(focused),
        }
    }
}

impl AccessibilityHost for OneWindowHost {
    fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R {
        read(&self.windows)
    }

    fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>> {
        self.focus.subscribe()
    }

    fn may_read(&self, _: &WindowRecord, _: &str) -> bool {
        true
    }

    fn flag_sensitive(&self, _: &WindowIdent) {}
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let cli = Cli::parse();
    let pid = match cli.mode {
        Mode::Listen => None,
        Mode::Read { pid } => Some(pid),
        Mode::Probe {
            bus_name,
            budgets_ms,
        } => return probe(&cli.bus, &bus_name, &budgets_ms),
    };
    let (outlet, observations) = mpsc::sync_channel(256);
    let _collector = AccessibilityCollector::spawn(
        outlet,
        OneWindowHost::new(pid),
        AccessibilityBus::Address(cli.bus),
        RecentReadings::default(),
    )
    .context("starting the collector")?;
    for observation in observations {
        if let ObservationKind::Text { lines, url, .. } = observation.kind {
            let bytes: usize = lines.iter().map(String::len).sum();
            println!(
                "text lines={} bytes={bytes} url={}",
                lines.len(),
                url.is_some()
            );
        }
    }
    Ok(())
}

fn probe(bus: &str, bus_name: &str, budgets_ms: &[u64]) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let connection = atspi::AccessibilityConnection::from_address(bus.try_into()?)
            .await?
            .connection()
            .clone();
        for budget in budgets_ms {
            let started = std::time::Instant::now();
            let reading = read_active_window(
                connection.clone(),
                bus_name,
                std::time::Duration::from_millis(*budget),
                None,
            )
            .await;
            let elapsed = started.elapsed().as_millis();
            match reading {
                Ok(reading) => {
                    let content = match &reading.content {
                        FrameContent::Text { lines, .. } => format!("text lines={}", lines.len()),
                        other => format!("{other:?}"),
                    };
                    println!(
                        "budget={budget}ms elapsed={elapsed}ms toolkit={:?} {content} calls={}",
                        reading.toolkit,
                        reading.calls.len()
                    );
                }
                Err(error) => println!("budget={budget}ms elapsed={elapsed}ms error={error}"),
            }
        }
        Ok(())
    })
}
