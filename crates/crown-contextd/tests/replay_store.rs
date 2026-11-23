//! A replayed trace has no compositor to grant the engine its scope. Its text and capture
//! records must still pass the content gate and reach the store.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::bail;
use context_core::{FrameRecord, Observation, ObservationKind, Trigger};
use context_eval::{Scenario, Trace, generate};
use context_store::Store;

const STARTUP: Duration = Duration::from_secs(30);
const NO_SESSION_BUS: &str = "unix:path=/nonexistent/bus";

struct KilledOnDrop(Child);

impl Drop for KilledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn row_count(state: &std::path::Path, table: &str) -> anyhow::Result<i64> {
    let store = Store::open_dir(state)?;
    Ok(store
        .connection()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?)
}

/// A synthetic day whose text observations carry lines, each with the record of the capture
/// that read them.
fn trace_with_text() -> Trace {
    let mut trace = generate(Scenario::Short, 7);
    trace.observations = trace
        .observations
        .into_iter()
        .flat_map(|mut observation| {
            let ObservationKind::Text { window, lines, .. } = &mut observation.kind else {
                return vec![observation];
            };
            lines.push("Replayed line of the synthetic day".to_owned());
            let frame = Observation::new(
                observation.t,
                ObservationKind::Frame(FrameRecord {
                    window: window.clone(),
                    trigger: Trigger::FocusChange,
                    width: 800,
                    height: 600,
                    damage_permille: 1000,
                    line_count: 1,
                    secret_seen: false,
                }),
            );
            vec![observation, frame]
        })
        .collect();
    trace
}

#[test]
fn replayed_text_and_frames_reach_the_store() -> anyhow::Result<()> {
    let scratch = PathBuf::from(format!("/tmp/vcge-replay-{}", std::process::id()));
    let config = scratch.join("config");
    let state = scratch.join("state");
    let sockets = scratch.join("rt");
    for directory in [&config, &state, &sockets] {
        std::fs::create_dir_all(directory)?;
    }
    let trace = scratch.join("trace.json");
    std::fs::write(&trace, trace_with_text().to_json()?)?;

    let _daemon = KilledOnDrop(
        Command::new(env!("CARGO_BIN_EXE_crown-contextd"))
            .arg("--replay")
            .arg(&trace)
            .args(["--speed", "0"])
            .arg("--state-dir")
            .arg(&state)
            .arg("--socket-dir")
            .arg(&sockets)
            .env("CROWN_CONFIG_DIR", &config)
            .env("XDG_RUNTIME_DIR", &sockets)
            .env("DBUS_SESSION_BUS_ADDRESS", NO_SESSION_BUS)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let started = Instant::now();
    while started.elapsed() < STARTUP {
        let stored = |table| row_count(&state, table).is_ok_and(|count| count > 0);
        if stored("text_lines") && stored("frames") {
            let _ = std::fs::remove_dir_all(&scratch);
            return Ok(());
        }
        sleep(Duration::from_millis(200));
    }
    bail!("no replayed text or capture record reached the store")
}
