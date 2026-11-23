#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Analyse, then delete, end to end. A nested crownpositor runs a window of seeded colour blocks for two
//! minutes while crown-contextd captures it. Then the state directory, the runtime directories,
//! `/tmp`, `/dev/shm` and every database value must hold no run of those pixels and no image.
//!
//! The compositor gives the Engine scope only to a root-owned `/usr/bin/crown-contextd`, so
//! the tests run in a user, mount and pid namespace where an overlay puts this build there. In
//! that namespace every file of the user looks owned by root, and the real root's files look
//! owned by nobody. Run the tests through `run-in-namespace.sh`, with the compositor binary in
//! `VCGE_NESTED_COMPOSITOR`.

mod fake_inferd;
mod integrated;
mod pixel_search;
mod test_window;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use context_eval::pixel_scan::PixelNeedles;
use context_store::Store;
use rustix::process::{Pid, Signal};

use pixel_search::{Findings, Search};

const RUN_LENGTH: Duration = Duration::from_secs(120);
/// The one executable that may hold the Engine scope.
const ENGINE: &str = "/usr/bin/crown-contextd";
/// No X11 bridge, so the nested session does not take an X display, which every session on the
/// machine shares.
const NO_X11_BRIDGE: &str = "/nonexistent/xwayland-satellite";
/// The test runs its window in a second process of itself, without `WORKSPACE`, so the window
/// opens on the nested compositor's own output and gets the focus.
const WINDOW_SOCKET_ENV: &str = "VCGE_TEST_WINDOW_SOCKET";
const ROUND_LENGTH: Duration = Duration::from_secs(15);
/// When the positive control is captured: the window shows by then.
const CONTROL_AFTER: Duration = Duration::from_secs(10);
/// Set by `run-in-namespace.sh` to a directory it made with `mkdtemp`.
const TEST_ROOT_ENV: &str = "VCGE_TEST_ROOT";

const fn seed_for(round: u64) -> u64 {
    0x5643_4745 ^ round.wrapping_mul(0x9E37_79B9)
}

/// A child process stopped by its own PID when the test ends, never by name.
struct Process {
    name: &'static str,
    child: Child,
}

impl Process {
    fn start(name: &'static str, command: &mut Command, log: &Path) -> Self {
        let log = fs::File::create(log).unwrap();
        let child = command
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {name}: {error}"));
        eprintln!("{name} runs as PID {}", child.id());
        Self { name, child }
    }

    /// Returns whether the process stopped within `grace`.
    fn interrupt_then_wait(&mut self, grace: Duration) -> bool {
        if let Some(pid) = Pid::from_raw(i32::try_from(self.child.id()).unwrap()) {
            let _ = rustix::process::kill_process(pid, Signal::INT);
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!(
            "{} did not stop in time; killing PID {}",
            self.name,
            self.child.id()
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
        false
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The log as plain text: the daemon colours its fields with ANSI escapes.
fn without_colours(log: &str) -> String {
    let mut plain = String::with_capacity(log.len());
    let mut in_escape = false;
    for character in log.chars() {
        match (in_escape, character) {
            (false, '\u{1b}') => in_escape = true,
            (true, 'm') => in_escape = false,
            (false, _) => plain.push(character),
            (true, _) => {}
        }
    }
    plain
}

fn parent_display() -> PathBuf {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    let display = PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY"));
    if display.is_absolute() {
        display
    } else {
        runtime.join(display)
    }
}

fn private_directory(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    rustix::fs::chmod(path, rustix::fs::Mode::from_raw_mode(0o700)).unwrap();
    path.to_owned()
}

fn wait_for_socket(runtime: &Path, timeout: Duration) -> PathBuf {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let socket = fs::read_dir(runtime).unwrap().flatten().find_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with("wayland-") && !name.ends_with(".lock")).then(|| entry.path())
        });
        if let Some(socket) = socket {
            return socket;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!(
        "the nested compositor made no socket in {}",
        runtime.display()
    );
}

fn assert_engine_is_this_build() {
    let built = fs::read(env!("CARGO_BIN_EXE_crown-contextd")).unwrap();
    assert!(
        fs::read(ENGINE).is_ok_and(|installed| installed == built),
        "{ENGINE} is not this build; run the test through run-in-namespace.sh"
    );
}

/// `agents.ron` names the engine, so the compositor migrates its Engine grant at startup.
fn write_config(config: &Path) {
    fs::write(
        config.join("agents.ron"),
        format!("(trusted_executables: [{ENGINE:?}], ask_for_app_access: false, app_access: {{}})"),
    )
    .unwrap();
    fs::write(config.join("visual_context.ron"), "(enabled: true)").unwrap();
}

fn needles_for_all_rounds(run_length: Duration) -> PixelNeedles {
    let mut needles = PixelNeedles::default();
    let rounds = run_length.as_secs() / ROUND_LENGTH.as_secs() + 2;
    for round in 0..rounds {
        needles.add_seed(seed_for(round));
    }
    needles
}

#[test]
#[ignore = "needs a crownpositor binary in VCGE_NESTED_COMPOSITOR and a Wayland session"]
fn no_pixels_reach_disk_after_two_minutes_of_capture() {
    let compositor_binary = PathBuf::from(
        std::env::var_os("VCGE_NESTED_COMPOSITOR").expect("set VCGE_NESTED_COMPOSITOR"),
    );
    assert_engine_is_this_build();
    let root = test_root();
    let runtime = private_directory(&root.join("runtime"));
    let config = private_directory(&root.join("config"));
    let state = private_directory(&root.join("state"));
    write_config(&config);
    let started = SystemTime::now();

    let mut compositor = Process::start(
        "crownpositor",
        Command::new(&compositor_binary)
            .env("CROWN_BACKEND", "winit")
            .env("WORKSPACE", "headless")
            .env("CROWN_XWAYLAND_SATELLITE", NO_X11_BRIDGE)
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("AT_SPI_BUS_ADDRESS")
            .env("CROWN_CONFIG_DIR", &config)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("WAYLAND_DISPLAY", parent_display()),
        &root.join("compositor.log"),
    );
    let socket = wait_for_socket(&runtime, Duration::from_secs(30));
    eprintln!("the nested compositor listens on {}", socket.display());
    let services = private_directory(&runtime.join("crownos"));
    let ocr_calls = fake_inferd::start(&services, 2);

    let mut window = Process::start(
        "test window",
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "test_window_process", "--nocapture"])
            .env(WINDOW_SOCKET_ENV, &socket)
            .env_remove("WORKSPACE"),
        &root.join("window.log"),
    );
    std::thread::sleep(Duration::from_secs(2));
    let mut contextd = Process::start(
        "crown-contextd",
        Command::new(ENGINE)
            .arg("--state-dir")
            .arg(&state)
            .arg("--socket-dir")
            .arg(&services)
            .env_remove("WORKSPACE")
            .env_remove("LD_PRELOAD")
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("AT_SPI_BUS_ADDRESS")
            .env("WAYLAND_DISPLAY", &socket)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("CROWN_CONFIG_DIR", &config)
            .env(
                "RUST_LOG",
                "info,context_capture=debug,crown_contextd=debug",
            ),
        &root.join("contextd.log"),
    );

    std::thread::sleep(CONTROL_AFTER);
    let control = capture_positive_control(&runtime, &socket, &root);
    std::thread::sleep(RUN_LENGTH - CONTROL_AFTER);
    let contextd_stopped = contextd.interrupt_then_wait(Duration::from_secs(15));
    window.interrupt_then_wait(Duration::from_secs(2));
    compositor.interrupt_then_wait(Duration::from_secs(10));
    assert!(
        contextd_stopped,
        "contextd did not stop on SIGINT; see {}",
        root.join("contextd.log").display()
    );

    let log = without_colours(&fs::read_to_string(root.join("contextd.log")).unwrap());
    assert!(
        log.contains("capture session bound"),
        "contextd never bound the capture globals; see {}",
        root.join("contextd.log").display()
    );
    let frames = Store::open_dir(&state).unwrap().count("frames").unwrap();
    eprintln!("frame rows: {frames}");
    assert!(frames > 0, "no capture was analysed");

    let ocr_calls = ocr_calls.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!("OCR calls to the fake inferd: {ocr_calls}");
    assert!(ocr_calls > 2, "the planted secret was never read");
    assert!(
        log.contains("a secret was on screen") && log.contains("flagged=true"),
        "the secret did not flag the window; see the contextd log"
    );
    let store = Store::open_dir(&state).unwrap();
    let stored_lines: Vec<String> = store
        .connection()
        .prepare("SELECT text FROM text_lines")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    eprintln!("stored lines: {stored_lines:?}");
    assert!(
        stored_lines
            .iter()
            .any(|line| line == fake_inferd::HARMLESS_LINE)
    );
    assert!(
        stored_lines
            .iter()
            .all(|line| !line.contains("AKIA") && line != fake_inferd::SECRET_LINE)
    );
    drop(store);

    assert_no_pixels_on_disk(&root, &state, started, RUN_LENGTH, &control);
    let _ = fs::remove_dir_all(&root);
}

/// The directory the test owns, made by `run-in-namespace.sh` with `mkdtemp`.
fn test_root() -> PathBuf {
    PathBuf::from(std::env::var_os(TEST_ROOT_ENV).expect("run through run-in-namespace.sh"))
}

/// A frame of the nested output with the test window on it, captured the way any host app
/// can, written as a PPM to `root`.
fn capture_positive_control(runtime: &Path, display: &Path, root: &Path) -> PathBuf {
    let control = root.join("positive-control.ppm");
    let status = Command::new("grim")
        .args(["-t", "ppm"])
        .arg(&control)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("WAYLAND_DISPLAY", display)
        .status()
        .unwrap();
    assert!(status.success(), "grim could not capture the nested output");
    control
}

/// No run of the test window's pixels and no image in the state directory, its database, the
/// runtime directories, `/tmp` or `/dev/shm`. The search must first find the window in a real
/// capture of the output, `control`.
fn assert_no_pixels_on_disk(
    root: &Path,
    state: &Path,
    started: SystemTime,
    run_length: Duration,
    control: &Path,
) {
    let needles = needles_for_all_rounds(run_length);
    let mut planted = Findings::default();
    let own = [root.to_owned()];
    let search = Search {
        needles: &needles,
        changed_since: started,
        own_directories: &own,
    };
    search.directory(root, &mut planted);
    assert_eq!(
        planted.pixel_runs,
        [control],
        "the search finds the window in a capture of the output"
    );
    fs::remove_file(control).unwrap();

    let mut findings = Findings::default();
    let values = pixel_search::database(state, &needles, &mut findings);
    for directory in [
        root.to_owned(),
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()),
        PathBuf::from("/tmp"),
        PathBuf::from("/dev/shm"),
    ] {
        search.directory(&directory, &mut findings);
    }
    eprintln!(
        "searched {values} database values and {} files changed during the run",
        findings.files_scanned
    );
    assert!(findings.is_clean(), "frame data on disk: {findings:?}");
}

#[test]
#[ignore = "the window process of the nested capture test"]
fn test_window_process() {
    let Some(socket) = std::env::var_os(WINDOW_SOCKET_ENV) else {
        return;
    };
    if let Err(error) = test_window::show_until_killed(Path::new(&socket), ROUND_LENGTH, seed_for) {
        panic!("the test window failed: {error}");
    }
}
