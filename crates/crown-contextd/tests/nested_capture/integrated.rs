//! The integrated engine, end to end. crown-contextd runs as `/usr/bin/crown-contextd` with the
//! Engine grant the compositor migrates from `agents.ron`. A noise window without an a11y tree
//! gets OCR text from the fake inferd; a GTK 4 window gets a11y text and no OCR; a secret the
//! GTK window shows later is dropped and flags the window. A second context client with only
//! the metadata scope sees no titles, and loses its grant when `context_access.ron` drops it.
//! After two minutes no pixel of the noise window is on disk or in the database.
//!
//! Run it through `run-in-namespace.sh`.

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{Receiver, channel};

use super::*;

const PROBE_ENV: &str = "VCGE_CONTEXT_PROBE";
const RUN_LENGTH: Duration = Duration::from_secs(150);
const NOISE_PHASE: Duration = Duration::from_secs(40);
const SECRET_AFTER: Duration = Duration::from_secs(15);
const GTK_PHASE: Duration = Duration::from_secs(35);
const STARTUP: Duration = Duration::from_secs(60);
/// `engine` in the protocol scope of a bound client.
const ENGINE_SCOPE_BIT: u32 = 8;
/// `Scope::Engine` in the compositor's audit line, a `ScopeSet` of crownos-config.
const GRANT_ENGINE_BIT: u32 = 1 << 7;

const GTK_APP_ID: &str = "org.crownos.VcgeIntegratedProbe";
const GTK_TITLE: &str = "vcge integrated probe";
const GTK_LABEL: &str = "Integrated probe label read through AT-SPI";
const GTK_BODY: &str = "Quarterly review notes that only the text view holds";
const GTK_PROBE: &str = r#"
import sys, gi
gi.require_version("Gtk", "4.0")
from gi.repository import GLib, Gtk
app_id, title, label, body, secret, secret_after = sys.argv[1:7]
def build(app):
    window = Gtk.ApplicationWindow(application=app, title=title)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    box.append(Gtk.Label(label=label))
    view = Gtk.TextView()
    view.get_buffer().set_text(body)
    box.append(view)
    window.set_child(box)
    window.present()
    GLib.timeout_add(int(secret_after), lambda: view.get_buffer().set_text(secret) and False)
app = Gtk.Application(application_id=app_id)
app.connect("activate", build)
app.run([])
"#;

/// The private buses and directories every process of the test shares.
struct Session {
    runtime: PathBuf,
    config: PathBuf,
    bus: String,
}

impl Session {
    /// A command that sees this session only: no user bus, no a11y bus of the desktop, no
    /// preloaded code.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("CROWN_CONFIG_DIR", &self.config)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus)
            .env("GSETTINGS_BACKEND", "memory")
            .env_remove("AT_SPI_BUS_ADDRESS")
            .env_remove("LD_PRELOAD")
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("DISPLAY");
        command
    }

    /// A command for a client of the nested compositor.
    fn client(&self, program: impl AsRef<std::ffi::OsStr>, display: &Path) -> Command {
        let mut command = self.command(program);
        command
            .env("WAYLAND_DISPLAY", display)
            .env_remove("WORKSPACE");
        command
    }
}

fn private_bus(runtime: &Path) -> (Process, String) {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .env("XDG_RUNTIME_DIR", runtime)
        .env("GSETTINGS_BACKEND", "memory")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    eprintln!("dbus-daemon runs as PID {}", child.id());
    let stdout = child.stdout.take().unwrap();
    let mut address = String::new();
    BufReader::new(stdout).read_line(&mut address).unwrap();
    let address = address.trim().to_owned();
    assert!(!address.contains("/run/user/"), "{address} is a live bus");
    let daemon = Process {
        name: "dbus-daemon",
        child,
    };
    (daemon, address)
}

/// Starts the a11y registry on the bus the session bus announces. The a11y bus would ask the
/// user's systemd for it, which knows nothing of this session.
fn start_a11y_registry(session: &Session, log: &Path) -> Process {
    let reply = session
        .command("dbus-send")
        .args([
            "--session",
            "--print-reply=literal",
            "--dest=org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Bus.GetAddress",
        ])
        .output()
        .unwrap();
    let address = String::from_utf8(reply.stdout).unwrap().trim().to_owned();
    assert!(
        address.contains(&*session.runtime.to_string_lossy()),
        "the a11y bus {address} left the private runtime directory"
    );
    Process::start(
        "at-spi2-registryd",
        session
            .command("/usr/lib/at-spi2-registryd")
            .env("AT_SPI_BUS_ADDRESS", &address),
        log,
    )
}

/// A context client whose output lines arrive on a channel.
struct Probe {
    process: Process,
    lines: Receiver<String>,
    heard: Vec<String>,
}

impl Probe {
    fn start(session: &Session, program: &Path, display: &Path) -> Self {
        let mut child = session
            .client(program, display)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        eprintln!("context probe runs as PID {}", child.id());
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = send.send(line);
            }
        });
        Self {
            process: Process {
                name: "context probe",
                child,
            },
            lines,
            heard: Vec::new(),
        }
    }

    fn heard(&mut self) -> &[String] {
        self.heard.extend(self.lines.try_iter());
        &self.heard
    }

    fn has_heard(&mut self, wanted: &str) -> bool {
        self.heard().iter().any(|line| line == wanted)
    }
}

fn wait_until(what: &str, timeout: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out waiting for {what}");
}

fn grants(probe: &Path, probe_scopes: &str, migrated: bool) -> String {
    let probe_grant = if probe_scopes.is_empty() {
        String::new()
    } else {
        format!(
            "{:?}: (scopes: [{probe_scopes}], expires: Never), ",
            format!("exe:{}", probe.display())
        )
    };
    let engine_grant = if migrated {
        format!("\"exe:{ENGINE}\": (scopes: [Engine], expires: Never)")
    } else {
        String::new()
    };
    format!(
        "(ask_on_first_use: false, host_default: Deny, migrated_agents: {migrated}, \
         grants: {{{probe_grant}{engine_grant}}})\n"
    )
}

fn bound_scope_set(audit_line: &str) -> Option<u32> {
    audit_line
        .split("ScopeSet(")
        .nth(1)?
        .split(')')
        .next()?
        .parse()
        .ok()
}

fn granted_scopes(contextd_log: &str) -> Vec<u32> {
    contextd_log
        .lines()
        .filter(|line| line.contains("the compositor granted a context scope"))
        .filter_map(|line| {
            line.split("scope=")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

fn frame_rows(state: &Path) -> Vec<(u32, u32, u32)> {
    let store = Store::open_dir(state).unwrap();
    store
        .connection()
        .prepare("SELECT width, height, line_count FROM frames")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn stored_lines(state: &Path) -> Vec<(String, String)> {
    let store = Store::open_dir(state).unwrap();
    store
        .connection()
        .prepare("SELECT source, text FROM text_lines")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
#[ignore = "run through run-in-namespace.sh: needs a user namespace, a nested crownpositor, \
            dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
fn integrated_engine_end_to_end() {
    let compositor_binary = PathBuf::from(
        std::env::var_os("VCGE_NESTED_COMPOSITOR").expect("set VCGE_NESTED_COMPOSITOR"),
    );
    let probe_binary = PathBuf::from(std::env::var_os(PROBE_ENV).expect("set VCGE_CONTEXT_PROBE"));
    assert_engine_is_this_build();

    let root = PathBuf::from(format!("/tmp/w8e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let runtime = private_directory(&root.join("run"));
    let config = private_directory(&root.join("config"));
    let state = private_directory(&root.join("state"));
    let programs = private_directory(&root.join("programs"));
    let probe_copy = programs.join("metadata-probe");
    fs::copy(&probe_binary, &probe_copy).unwrap();
    write_config(&config);
    let access = config.join("context_access.ron");
    fs::write(&access, grants(&probe_copy, "Metadata", false)).unwrap();
    let started = SystemTime::now();

    let (_bus, bus) = private_bus(&runtime);
    let session = Session {
        runtime: runtime.clone(),
        config,
        bus,
    };
    let mut compositor = Process::start(
        "crownpositor",
        session
            .command(&compositor_binary)
            .env("CROWN_BACKEND", "winit")
            .env("WORKSPACE", "headless")
            .env("CROWN_XWAYLAND_SATELLITE", NO_X11_BRIDGE)
            .env("WAYLAND_DISPLAY", parent_display()),
        &root.join("compositor.log"),
    );
    let display = wait_for_socket(&runtime, Duration::from_secs(30));
    eprintln!("the nested compositor listens on {}", display.display());
    let _registry = start_a11y_registry(&session, &root.join("registry.log"));
    let services = private_directory(&runtime.join("crownos"));
    let ocr_calls = fake_inferd::start(&services, usize::MAX);
    let ocr_count = || ocr_calls.load(std::sync::atomic::Ordering::SeqCst);

    let mut metadata_probe = Probe::start(&session, &probe_copy, &display);
    let mut noise = Process::start(
        "noise window",
        session
            .client(std::env::current_exe().unwrap(), &display)
            .args(["--ignored", "--exact", "test_window_process", "--nocapture"])
            .env(WINDOW_SOCKET_ENV, &display),
        &root.join("window.log"),
    );
    std::thread::sleep(Duration::from_secs(2));
    let contextd_started = Instant::now();
    let mut contextd = Process::start(
        "crown-contextd",
        session
            .client(ENGINE, &display)
            .arg("--state-dir")
            .arg(&state)
            .arg("--socket-dir")
            .arg(&services)
            .env(
                "RUST_LOG",
                "info,context_capture=debug,crown_contextd=debug,context_collect=debug",
            ),
        &root.join("contextd.log"),
    );

    std::thread::sleep(NOISE_PHASE);
    let ocr_before_gtk = ocr_count();
    eprintln!("OCR calls while the noise window had the focus: {ocr_before_gtk}");
    let secret_after = SECRET_AFTER.as_millis().to_string();
    let mut gtk = Process::start(
        "gtk4 probe",
        session.client("python3", &display).args([
            "-c",
            GTK_PROBE,
            GTK_APP_ID,
            GTK_TITLE,
            GTK_LABEL,
            GTK_BODY,
            fake_inferd::SECRET_LINE,
            &secret_after,
        ]),
        &root.join("gtk.log"),
    );
    std::thread::sleep(GTK_PHASE);
    let ocr_after_gtk = ocr_count();
    eprintln!("OCR calls after the GTK window had the focus: {ocr_after_gtk}");

    let app_line = format!("app_id {GTK_APP_ID}");
    wait_until("the metadata probe to see the GTK window", STARTUP, || {
        metadata_probe.has_heard(&app_line)
    });
    fs::write(&access, grants(&probe_copy, "", true)).unwrap();
    wait_until("the revoke to reach the metadata probe", STARTUP, || {
        metadata_probe.has_heard("revoked")
    });

    if let Some(left) = RUN_LENGTH.checked_sub(contextd_started.elapsed()) {
        std::thread::sleep(left);
    }
    contextd.interrupt_then_wait(Duration::from_secs(15));
    gtk.interrupt_then_wait(Duration::from_secs(2));
    noise.interrupt_then_wait(Duration::from_secs(2));
    metadata_probe
        .process
        .interrupt_then_wait(Duration::from_secs(2));
    compositor.interrupt_then_wait(Duration::from_secs(10));

    let compositor_log = without_colours(&fs::read_to_string(root.join("compositor.log")).unwrap());
    let contextd_log = without_colours(&fs::read_to_string(root.join("contextd.log")).unwrap());
    let engine_actor = format!("actor=exe:{ENGINE}");
    let engine_bound = compositor_log.lines().any(|line| {
        line.contains("bound")
            && line.contains(&engine_actor)
            && bound_scope_set(line).is_some_and(|scopes| scopes & GRANT_ENGINE_BIT != 0)
    });
    assert!(
        engine_bound,
        "the compositor did not bind the engine with its Engine grant; see {}",
        root.join("compositor.log").display()
    );
    let granted = granted_scopes(&contextd_log);
    eprintln!("scopes the engine was granted: {granted:?}");
    assert!(
        granted.iter().any(|scope| scope & ENGINE_SCOPE_BIT != 0),
        "contextd did not get the engine scope"
    );
    assert!(
        !contextd_log.contains("withdrew the context grant"),
        "the revoke of the probe reached the engine"
    );
    let migrated = fs::read_to_string(&access).unwrap();
    eprintln!("context_access.ron after the run: {migrated}");

    let lines = stored_lines(&state);
    eprintln!("stored lines: {lines:?}");
    assert!(
        lines
            .iter()
            .any(|(source, text)| source == "ocr" && text == fake_inferd::HARMLESS_LINE),
        "no OCR text from the noise window"
    );
    assert!(
        lines
            .iter()
            .any(|(source, text)| source == "a11y" && text.contains(GTK_LABEL)),
        "no a11y text from the GTK window"
    );
    assert_eq!(
        ocr_after_gtk, ocr_before_gtk,
        "OCR ran while the GTK window with a11y text had the focus"
    );
    let gtk_frames: Vec<(u32, u32, u32)> = frame_rows(&state)
        .into_iter()
        .filter(|(width, height, _)| (*width, *height) != (WIDTH, HEIGHT))
        .collect();
    eprintln!("frames of the GTK window (width, height, lines): {gtk_frames:?}");
    assert!(
        gtk_frames.iter().any(|(_, _, lines)| *lines > 0),
        "no capture of the GTK window took its a11y text"
    );
    assert!(
        lines.iter().all(|(_, text)| !text.contains("AKIA")),
        "the planted secret reached the store"
    );
    assert!(
        contextd_log.contains("a secret was on screen") && contextd_log.contains("flagged=true"),
        "the secret did not flag the window; see {}",
        root.join("contextd.log").display()
    );

    let heard = metadata_probe.heard().to_vec();
    eprintln!("the metadata probe heard: {heard:?}");
    assert!(heard.iter().any(|line| line == "scope"));
    assert!(!heard.iter().any(|line| line == "title"), "{heard:?}");

    assert_no_pixels_on_disk(&root, &state, started, RUN_LENGTH);
    let _ = fs::remove_dir_all(&root);
}
