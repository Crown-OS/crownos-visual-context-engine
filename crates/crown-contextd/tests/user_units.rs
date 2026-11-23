#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The packaged user units, run by a private `systemd --user`. crown-contextd identifies the
//! program of an API caller; a unit option that makes a user namespace would hide it, as
//! `PrivateTmp=yes` shows. crown-context-inferd serves processes of the user from inside its
//! file system sandbox, which closes crown-contextd's state directory, database and key.
//!
//! The manager has its own runtime directory, unit path, configuration and cgroup, so it never
//! touches the session's user manager or its units. It is stopped by its PID.
//!
//! ```sh
//! cargo build -p crown-context-inferd
//! cargo test -p crown-contextd --test user_units -- --ignored
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use context_protocol::context::{Client as ContextClient, status};
use context_protocol::inference::Client as InferenceClient;
use context_store::{DATABASE_FILE, KEY_FILE};
use crownos_ipc::{Error, RemoteError};

const TIMEOUT: Option<Duration> = Some(Duration::from_secs(10));
const STARTUP: Duration = Duration::from_secs(30);
const USER_MANAGER: &str = "/usr/lib/systemd/systemd";
/// The inferd unit with a shell in place of inferd, to try what inferd could open.
const PROBE: &str = "crown-context-inferd-probe.service";
/// What the probe tries inside the sandbox: each line prints `open <what>` or `closed <what>`.
const PROBE_SCRIPT: &str = r#"
check() { if [ "$1" = dir ]; then ls "$3" >/dev/null 2>&1; else cat "$3" >/dev/null 2>&1; fi && echo "open $2" || echo "closed $2"; }
check dir state "$STATE"
check file database "$STATE/$DATABASE"
check file key "$STATE/$KEY"
touch "$CACHE/probe" 2>/dev/null && echo "open cache" || echo "closed cache"
"#;
const TARGETS: [&str; 5] = [
    "default",
    "basic",
    "shutdown",
    "sockets",
    "graphical-session",
];

fn packaged_unit(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/systemd")
        .join(name);
    fs::read_to_string(path).unwrap()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + STARTUP;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out waiting for {what}");
}

/// A private user manager in a cgroup of its own, below the test's.
struct UserManager {
    root: PathBuf,
    runtime: PathBuf,
    units: PathBuf,
    cgroup: PathBuf,
    manager: Child,
}

impl UserManager {
    fn start(root: &Path) -> Self {
        let runtime = root.join("run");
        let units = root.join("units");
        for directory in [&runtime, &units, &root.join("home")] {
            fs::create_dir_all(directory).unwrap();
        }
        rustix::fs::chmod(&runtime, rustix::fs::Mode::from_raw_mode(0o700)).unwrap();
        for target in TARGETS {
            fs::write(
                units.join(format!("{target}.target")),
                format!("[Unit]\nDescription={target}\n"),
            )
            .unwrap();
        }
        let own = fs::read_to_string("/proc/self/cgroup").unwrap();
        let own = own.trim().strip_prefix("0::").unwrap();
        let parent = Path::new("/sys/fs/cgroup").join(own.trim_start_matches('/'));
        let cgroup = parent
            .parent()
            .unwrap()
            .join(format!("vcge-user-units-{}", std::process::id()));
        fs::create_dir(&cgroup).unwrap();
        let manager = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "echo $$ > {}/cgroup.procs && exec {USER_MANAGER} --user",
                cgroup.display()
            ))
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("HOME", root.join("home"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_CONFIG_HOME", root.join("home/.config"))
            .env("XDG_STATE_HOME", root.join("home/.local/state"))
            .env("XDG_CACHE_HOME", root.join("home/.cache"))
            .env("XDG_DATA_HOME", root.join("home/.local/share"))
            .env("SYSTEMD_UNIT_PATH", &units)
            .stdin(Stdio::null())
            .stdout(fs::File::create(root.join("manager.log")).unwrap())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        eprintln!("the private user manager runs as PID {}", manager.id());
        let private = runtime.join("systemd/private");
        wait_until("the user manager", || private.exists());
        Self {
            root: root.to_owned(),
            runtime,
            units,
            cgroup,
            manager,
        }
    }

    fn systemctl(&self, arguments: &[&str]) -> bool {
        Command::new("systemctl")
            .arg("--user")
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .status()
            .unwrap()
            .success()
    }

    /// Installs a packaged unit with a drop-in that points it at this build and a log file.
    fn install(&self, name: &str, service_lines: &str) {
        fs::write(self.units.join(name), packaged_unit(name)).unwrap();
        let drop_in = self.units.join(format!("{name}.d"));
        fs::create_dir_all(&drop_in).unwrap();
        if !service_lines.is_empty() {
            fs::write(
                drop_in.join("test.conf"),
                format!("[Service]\n{service_lines}"),
            )
            .unwrap();
        }
    }

    /// The packaged inferd unit, sandbox and all, running a shell that tries to read
    /// contextd's state directory, database and key, and to write the inferd cache.
    fn install_sandbox_probe(&self, state: &Path) {
        let script = self.root.join("probe.sh");
        fs::write(&script, PROBE_SCRIPT).unwrap();
        let unit = packaged_unit("crown-context-inferd.service")
            .replace("Type=simple", "Type=oneshot")
            .replace(
                "ExecStart=/usr/bin/crown-context-inferd",
                &format!("ExecStart=/usr/bin/sh {}", script.display()),
            );
        fs::write(self.units.join(PROBE), unit).unwrap();
        let log = self.root.join("probe.log");
        let drop_in = self.units.join(format!("{PROBE}.d"));
        fs::create_dir_all(&drop_in).unwrap();
        fs::write(
            drop_in.join("test.conf"),
            format!(
                "[Service]\nEnvironment=STATE={}\nEnvironment=DATABASE={DATABASE_FILE}\n\
                 Environment=KEY={KEY_FILE}\nEnvironment=CACHE=%C/crownos/context\n\
                 StandardOutput=append:{}\nStandardError=append:{}\n",
                state.display(),
                log.display(),
                log.display()
            ),
        )
        .unwrap();
        assert!(self.systemctl(&["daemon-reload"]));
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(name)).unwrap_or_default()
    }
}

impl Drop for UserManager {
    fn drop(&mut self) {
        self.systemctl(&[
            "stop",
            "crown-contextd.service",
            "crown-context-inferd.service",
            "crown-context-inferd.socket",
        ]);
        let _ = self.manager.kill();
        let _ = self.manager.wait();
        let mut cgroups: Vec<PathBuf> = walk(&self.cgroup);
        cgroups.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for cgroup in cgroups {
            let _ = fs::remove_dir(cgroup);
        }
    }
}

fn walk(directory: &Path) -> Vec<PathBuf> {
    let mut found = vec![directory.to_owned()];
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            found.extend(walk(&entry.path()));
        }
    }
    found
}

fn service_lines(
    binary: &Path,
    arguments: &str,
    environment: &[(&str, &Path)],
    log: &Path,
) -> String {
    let mut lines = format!(
        "ExecStart=\nExecStart={} {arguments}\nRestart=no\nStandardOutput=append:{}\nStandardError=append:{}\n",
        binary.display(),
        log.display(),
        log.display()
    );
    for (name, value) in environment {
        lines.push_str(&format!("Environment={name}={}\n", value.display()));
    }
    lines
}

/// The newest llama.cpp build of this target directory: a debug inferd has no installed
/// libraries next to it.
fn llama_libraries(binary: &Path) -> PathBuf {
    let build = binary.with_file_name("build");
    fs::read_dir(build)
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("llama-cpp-sys-2-")
        })
        .map(|entry| entry.path().join("out/lib"))
        .filter_map(|libraries| {
            let built = fs::metadata(libraries.join("libllama.so"))
                .ok()?
                .modified()
                .ok()?;
            Some((built, libraries))
        })
        .max_by_key(|(built, _)| *built)
        .map(|(_, libraries)| libraries)
        .expect("no llama.cpp build in the target directory")
}

fn status_of(runtime: &Path) -> Result<(), Error> {
    let mut client = ContextClient::connect_in(&runtime.join("crownos"))?;
    client
        .call_blocking::<status>(&status {}, Vec::new(), TIMEOUT)
        .map(|_| ())
}

#[test]
#[ignore = "needs systemd; starts a private user manager"]
fn the_packaged_units_identify_their_peers() {
    // Not in /tmp: the inferd unit has its own /tmp, which would hide the state directory for
    // a reason other than the one under test.
    let root =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("vcge-units-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let config = root.join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("visual_context.ron"), "(enabled: true)").unwrap();
    let this_test = std::env::current_exe().unwrap();
    fs::write(
        config.join("context_access.ron"),
        format!(
            "(ask_on_first_use: false, host_default: Deny, migrated_agents: true, \
             grants: {{{:?}: (scopes: [Metadata], expires: Never)}})\n",
            format!("exe:{}", this_test.display())
        ),
    )
    .unwrap();
    let contextd = PathBuf::from(env!("CARGO_BIN_EXE_crown-contextd"));
    let inferd = contextd.with_file_name("crown-context-inferd");
    assert!(inferd.is_file(), "build crown-context-inferd first");
    let registry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/registry.ron");

    let manager = UserManager::start(&root);
    let no_display = Path::new("/nonexistent/wayland-0");
    let no_bus = Path::new("unix:path=/nonexistent/bus");
    manager.install(
        "crown-contextd.service",
        &service_lines(
            &contextd,
            "",
            &[
                ("WAYLAND_DISPLAY", no_display),
                ("DBUS_SESSION_BUS_ADDRESS", no_bus),
                ("CROWN_CONFIG_DIR", &config),
            ],
            &root.join("contextd.log"),
        ),
    );
    manager.install(
        "crown-context-inferd.service",
        &service_lines(
            &inferd,
            &format!("--registry {}", registry.display()),
            &[
                ("LD_LIBRARY_PATH", &llama_libraries(&contextd)),
                ("CROWN_CONTEXT_MODELS", &root.join("models")),
                ("ORT_DYLIB_PATH", &root.join("no-onnxruntime.so")),
            ],
            &root.join("inferd.log"),
        ),
    );
    manager.install("crown-context-inferd.socket", "");
    assert!(manager.systemctl(&["daemon-reload"]));
    assert!(manager.systemctl(&["start", "crown-context-inferd.socket"]));
    assert!(manager.systemctl(&["start", "crown-contextd.service"]));

    let socket = manager
        .runtime
        .join(format!("crownos/{}.sock", context_protocol::SERVICE));
    wait_until("the context socket", || socket.exists());
    status_of(&manager.runtime).expect("contextd did not identify this test");
    eprintln!("contextd identified this test and answered");

    let mut client = InferenceClient::connect_in(&manager.runtime.join("crownos")).unwrap();
    let pending = client.capabilities().unwrap();
    client
        .wait(pending, TIMEOUT)
        .expect("inferd did not serve this test");
    eprintln!("inferd served a process of the user");

    let state = manager.root.join("home/.local/state/crownos/context");
    for file in [DATABASE_FILE, KEY_FILE] {
        assert!(state.join(file).is_file(), "contextd made no {file}");
    }
    manager.install_sandbox_probe(&state);
    assert!(manager.systemctl(&["start", PROBE]));
    let probe = manager.log("probe.log");
    eprintln!(
        "inside the inferd sandbox: {}",
        probe.trim().replace('\n', ", ")
    );
    for line in [
        "closed state",
        "closed database",
        "closed key",
        "open cache",
    ] {
        assert!(
            probe.lines().any(|seen| seen == line),
            "{line} missing: {probe}"
        );
    }

    fs::write(
        manager
            .units
            .join("crown-contextd.service.d/namespace.conf"),
        "[Service]\nPrivateTmp=yes\n",
    )
    .unwrap();
    assert!(manager.systemctl(&["daemon-reload"]));
    assert!(manager.systemctl(&["restart", "crown-contextd.service"]));
    std::thread::sleep(Duration::from_secs(2));
    wait_until("the context socket again", || socket.exists());
    match status_of(&manager.runtime) {
        Err(Error::Remote(error)) if error.code == RemoteError::UNAUTHORIZED => {
            eprintln!(
                "with PrivateTmp=yes, contextd could not identify this test: {}",
                error.message
            );
        }
        other => panic!("with PrivateTmp=yes the caller was still identified: {other:?}"),
    }
    drop(manager);
    let _ = Command::new("chmod")
        .arg("-R")
        .arg("u+rwx")
        .arg(&root)
        .status();
    let _ = fs::remove_dir_all(&root);
}
