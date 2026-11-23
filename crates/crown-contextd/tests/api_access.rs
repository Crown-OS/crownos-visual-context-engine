//! Runs crown-contextd on an empty replay with a scratch `context_access.ron`
//! and checks the Context API against it: a caller gets what its grant
//! allows, and removing the grant disconnects it.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use context_protocol::context::{Client, current_context, explain, status};
use context_protocol::{MemoryKind, MemoryRef};
use crownos_ipc::{Error, RemoteError};

const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));
const STARTUP: Duration = Duration::from_secs(30);
/// No portal can be reached, so a first-use question fails at once instead
/// of opening a dialog on the user's screen.
const NO_SESSION_BUS: &str = "unix:path=/nonexistent/bus";

struct KilledOnDrop(Child);

impl Drop for KilledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn grants(caller: &Path, scopes: &str) -> String {
    format!(
        "(host_default: Deny, migrated_agents: true, grants: {{\"exe:{}\": (scopes: [{scopes}], expires: Never)}})\n",
        caller.display()
    )
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) -> anyhow::Result<()> {
    let started = Instant::now();
    while started.elapsed() < STARTUP {
        if ready() {
            return Ok(());
        }
        sleep(Duration::from_millis(100));
    }
    bail!("timed out waiting for {what}")
}

#[test]
fn a_caller_gets_its_grant_and_loses_the_connection_with_it() -> anyhow::Result<()> {
    let scratch = PathBuf::from(format!("/tmp/w1b-ctxd-{}-grant", std::process::id()));
    let config = scratch.join("config");
    let state = scratch.join("state");
    let sockets = scratch.join("rt");
    for directory in [&config, &state, &sockets] {
        std::fs::create_dir_all(directory)?;
    }
    let trace = scratch.join("trace.json");
    std::fs::write(&trace, r#"{"seed":0,"observations":[],"gold":[]}"#)?;
    let access = config.join("context_access.ron");
    let this_test = std::env::current_exe()?;
    std::fs::write(&access, grants(&this_test, "Metadata"))?;

    let _daemon = KilledOnDrop(
        Command::new(env!("CARGO_BIN_EXE_crown-contextd"))
            .arg("--replay")
            .arg(&trace)
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
    let socket = sockets.join(format!("{}.sock", context_protocol::SERVICE));
    wait_until("the context socket", || socket.exists())?;

    let mut client = Client::connect_in(&sockets)?;
    client.call_blocking::<status>(&status {}, Vec::new(), TIMEOUT)?;
    client.call_blocking::<current_context>(&current_context {}, Vec::new(), TIMEOUT)?;
    let refused = client.call_blocking::<explain>(
        &explain {
            memory: MemoryRef {
                kind: MemoryKind::Segment,
                id: 1,
            },
        },
        Vec::new(),
        TIMEOUT,
    );
    match refused {
        Err(Error::Remote(error)) if error.code == RemoteError::UNAUTHORIZED => {}
        other => bail!("explain without the text scope gave {other:?}"),
    }

    std::fs::write(&access, "(host_default: Deny, migrated_agents: true)\n")?;
    wait_until("the revoked caller to be disconnected", || {
        matches!(
            client.call_blocking::<status>(
                &status {},
                Vec::new(),
                Some(Duration::from_millis(200))
            ),
            Err(Error::Disconnected | Error::Io(_))
        )
    })
    .context("the daemon kept a caller whose grant is gone")?;

    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

#[test]
fn a_caller_without_a_grant_waits_for_the_question_and_gets_a_clear_refusal() -> anyhow::Result<()>
{
    let scratch = PathBuf::from(format!("/tmp/w1b-ctxd-{}-ask", std::process::id()));
    let config = scratch.join("config");
    let state = scratch.join("state");
    let sockets = scratch.join("rt");
    for directory in [&config, &state, &sockets] {
        std::fs::create_dir_all(directory)?;
    }
    let trace = scratch.join("trace.json");
    std::fs::write(&trace, r#"{"seed":0,"observations":[],"gold":[]}"#)?;
    std::fs::write(
        config.join("context_access.ron"),
        "(migrated_agents: true)\n",
    )?;

    let _daemon = KilledOnDrop(
        Command::new(env!("CARGO_BIN_EXE_crown-contextd"))
            .arg("--replay")
            .arg(&trace)
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
    let socket = sockets.join(format!("{}.sock", context_protocol::SERVICE));
    wait_until("the context socket", || socket.exists())?;

    let mut client = Client::connect_in(&sockets)?;
    match client.call_blocking::<status>(&status {}, Vec::new(), TIMEOUT) {
        Err(Error::Remote(error))
            if error.code == RemoteError::UNAUTHORIZED
                && error.message.contains("nobody answered") => {}
        other => bail!("a caller without a grant got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}
