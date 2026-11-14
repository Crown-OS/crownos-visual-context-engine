//! `crown-context`: the engine from a terminal.

mod util;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use context_protocol::context::{Client, ContextChanged, Event, SegmentClosed};
use context_protocol::{
    ForgetRequest, Granularity, Redaction, ResultKind, SearchQuery, TimeSelector, TimelineQuery,
};

use crate::util::time::{clock, duration_text};

const REPLY_TIMEOUT: Option<Duration> = Some(Duration::from_secs(10));

#[derive(Debug, Parser)]
#[command(name = "crown-context", about, version)]
struct Cli {
    /// Print replies as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Whether the engine observes, and how it is doing.
    Status,
    /// What the user is doing right now.
    Context,
    /// Follow context changes and closed segments as they happen.
    Tail,
    /// Segments and episodes over a time range.
    Timeline {
        #[arg(long, conflicts_with_all = ["yesterday", "hours"])]
        today: bool,
        #[arg(long)]
        yesterday: bool,
        /// The last N hours.
        #[arg(long)]
        hours: Option<u32>,
        #[arg(long, default_value_t = 0.0)]
        min_importance: f32,
        /// Group by episode.
        #[arg(long)]
        episodes: bool,
    },
    /// Search the text seen on screen and the places visited.
    Search {
        text: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },
    /// Forget everything in a stretch of time.
    Forget {
        #[arg(long)]
        last_minutes: u32,
        #[arg(long)]
        app: Option<String>,
    },
    /// Stop observing; without seconds, until resumed.
    Pause {
        #[arg(long, default_value_t = 0)]
        seconds: u64,
    },
    /// Observe again.
    Resume,
}

fn connect() -> Result<Client> {
    Client::connect().context("crown-contextd is not running")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut client = connect()?;
    match cli.command {
        Command::Status => {
            let pending = client.status()?;
            let status = client.wait(pending, REPLY_TIMEOUT)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "{:?}{} · compositor {} · {} observations seen, {} dropped · {} open segment(s)",
                    status.state,
                    status
                        .pause_reason
                        .map(|reason| format!(" ({reason})"))
                        .unwrap_or_default(),
                    if status.compositor_connected {
                        "connected"
                    } else {
                        "absent"
                    },
                    status.observations_seen,
                    status.observations_dropped,
                    status.open_segments
                );
            }
        }
        Command::Context => {
            let pending = client.current_context()?;
            let context = client.wait(pending, REPLY_TIMEOUT)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&context)?);
            } else {
                print_context(&context);
            }
        }
        Command::Tail => {
            client.subscribe_blocking::<ContextChanged>(REPLY_TIMEOUT)?;
            client.subscribe_blocking::<SegmentClosed>(REPLY_TIMEOUT)?;
            loop {
                while let Some(event) = client.next_event() {
                    match event {
                        Event::ContextChanged(ContextChanged { context }) if cli.json => {
                            println!("{}", serde_json::to_string(&context)?);
                        }
                        Event::ContextChanged(ContextChanged { context }) => {
                            print_context(&context)
                        }
                        Event::SegmentClosed(SegmentClosed { segment }) if cli.json => {
                            println!("{}", serde_json::to_string(&segment)?);
                        }
                        Event::SegmentClosed(SegmentClosed { segment }) => {
                            println!(
                                "segment {} {}–{} {} {} {:?} {} active, importance {:.2}",
                                segment.id,
                                clock(segment.range.start),
                                clock(segment.range.end),
                                segment.app_key,
                                segment.doc_key,
                                segment.label,
                                duration_text(segment.active),
                                segment.importance
                            );
                        }
                        Event::CaptureStateChanged(changed) => {
                            println!("capture {:?}", changed.state)
                        }
                    }
                }
                util::wait::readable(&client)?;
                client.handle_readable()?;
            }
        }
        Command::Timeline {
            today: _,
            yesterday,
            hours,
            min_importance,
            episodes,
        } => {
            let time = match (yesterday, hours) {
                (true, _) => TimeSelector::Yesterday,
                (false, Some(hours)) => TimeSelector::LastHours(hours),
                (false, None) => TimeSelector::Today,
            };
            let pending = client.timeline(TimelineQuery {
                time,
                granularity: if episodes {
                    Granularity::Episode
                } else {
                    Granularity::Segment
                },
                min_importance,
                app_key: None,
            })?;
            let timeline = client.wait(pending, REPLY_TIMEOUT)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&timeline)?);
            } else if episodes {
                for episode in &timeline.episodes {
                    println!(
                        "{}–{} {:?} {} ({} segments)",
                        clock(episode.range.start),
                        clock(episode.range.end),
                        episode.label,
                        episode.title,
                        episode.segments.len()
                    );
                }
            } else {
                for segment in &timeline.segments {
                    println!(
                        "{}–{} {:<22} {:?} {}",
                        clock(segment.range.start),
                        clock(segment.range.end),
                        segment.app_key,
                        segment.label,
                        segment.title
                    );
                }
            }
        }
        Command::Search { text, limit } => {
            if text.is_empty() {
                bail!("search needs words");
            }
            let pending = client.search(SearchQuery {
                text: text.join(" "),
                time: None,
                app_key: None,
                kinds: vec![ResultKind::Segment],
                limit,
                redaction: Redaction::Snippets,
            })?;
            let results = client.wait(pending, REPLY_TIMEOUT)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for hit in &results.hits {
                    println!(
                        "{:.2} {} {} {} {}",
                        hit.score,
                        clock(hit.t_start),
                        hit.app_key,
                        hit.title,
                        hit.snippet.as_deref().unwrap_or("")
                    );
                }
                println!("{} hits in {} ms", results.hits.len(), results.took_ms);
            }
        }
        Command::Forget { last_minutes, app } => {
            let pending = client.forget(ForgetRequest {
                time: TimeSelector::LastMinutes(last_minutes),
                app_key: app,
                cascade: true,
            })?;
            let report = client.wait(pending, REPLY_TIMEOUT)?;
            println!("{}", serde_json::to_string(&report)?);
        }
        Command::Pause { seconds } => client.pause(seconds)?,
        Command::Resume => client.resume()?,
    }
    Ok(())
}

fn print_context(context: &context_protocol::CurrentContext) {
    match &context.focus {
        Some(focus) => println!(
            "{} {} — {} [{}]{}",
            clock(context.t),
            focus.app.name,
            focus.title,
            focus.doc_key,
            context
                .activity
                .as_ref()
                .map(|activity| format!(
                    " {:?} {:.0}%",
                    activity.label,
                    activity.confidence * 100.0
                ))
                .unwrap_or_default()
        ),
        None => println!("{} nothing focused", clock(context.t)),
    }
}
