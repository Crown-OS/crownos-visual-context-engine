//! A private session bus with its own a11y bus, and probe apps that start on it. The probes need
//! a Wayland display: run the tests inside a nested crownpositor.

#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use zbus::zvariant::Value;

pub const GTK4_TITLE: &str = "vcge a11y probe gtk4";
pub const QT_TITLE: &str = "vcge a11y probe qt";
pub const LABEL_TEXT: &str = "Probe label for the context engine";
pub const BODY_TEXT: &str = "Probe body text that only the text view holds";

pub const GTK4_PROBE: &str = r#"
import sys, gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk
title, label, body, password = sys.argv[1:5]
def build(app):
    window = Gtk.ApplicationWindow(application=app, title=title)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    box.append(Gtk.Label(label=label))
    view = Gtk.TextView()
    view.get_buffer().set_text(body)
    box.append(view)
    if password == "1":
        box.append(Gtk.PasswordEntry())
    window.set_child(box)
    window.present()
app = Gtk.Application(application_id="org.crownos.VcgeProbeGtk4")
app.connect("activate", build)
app.run([])
"#;

pub const QT_PROBE: &str = r#"
import sys
from PySide6.QtWidgets import QApplication, QLabel, QPlainTextEdit, QVBoxLayout, QWidget
title, label, body = sys.argv[1:4]
app = QApplication([])
window = QWidget()
window.setWindowTitle(title)
layout = QVBoxLayout(window)
layout.addWidget(QLabel(label))
editor = QPlainTextEdit()
editor.setPlainText(body)
layout.addWidget(editor)
window.show()
app.exec()
"#;

/// Kills its process when dropped; only the PID it started.
#[derive(Debug)]
pub struct Process(pub Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Process {
    pub fn pid(&self) -> u32 {
        self.0.id()
    }
}

/// A `dbus-daemon` of our own, so the test never touches the user's session or a11y bus. The
/// a11y bus launcher puts its socket in `$XDG_RUNTIME_DIR/at-spi`, so the session gets a runtime
/// directory of its own too: with the user's one it would replace the socket of the live a11y bus.
#[derive(Debug)]
pub struct PrivateSession {
    pub address: String,
    runtime_dir: std::path::PathBuf,
    registry: std::sync::Mutex<Option<Process>>,
    daemon: Process,
}

impl Drop for PrivateSession {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.take();
        }
        let _ = self.daemon.0.kill();
        let _ = self.daemon.0.wait();
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            let _ = std::fs::remove_dir_all(&self.runtime_dir);
            std::thread::sleep(Duration::from_millis(100));
            if !self.runtime_dir.exists() {
                break;
            }
        }
    }
}

impl PrivateSession {
    pub fn start() -> Self {
        static SESSIONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let runtime_dir = std::env::temp_dir().join(format!(
            "vcge-a11y-{}-{}",
            std::process::id(),
            SESSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env_remove("DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon is installed");
        let stdout = daemon.stdout.take().expect("piped stdout");
        let mut address = String::new();
        BufReader::new(stdout)
            .read_line(&mut address)
            .expect("dbus-daemon prints its address");
        Self {
            address: address.trim().to_owned(),
            runtime_dir,
            registry: std::sync::Mutex::new(None),
            daemon: Process(daemon),
        }
    }

    /// Starts the a11y bus launcher on this session, turns accessibility on, starts the
    /// registry and gives the a11y bus address. The registry is started by hand: the a11y bus
    /// would ask the user's systemd for it, which knows nothing of this session.
    pub async fn enable_accessibility(&self) -> String {
        let session = zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &("org.a11y.Status", "IsEnabled", Value::from(true)),
            )
            .await
            .unwrap();
        let reply = session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            )
            .await
            .unwrap();
        let a11y: String = reply.body().deserialize().unwrap();
        let private_dir = self.runtime_dir.to_string_lossy();
        assert!(
            a11y.contains(private_dir.as_ref()),
            "the a11y bus {a11y} left the private runtime directory {private_dir}"
        );
        let registry = Command::new("/usr/lib/at-spi2-registryd")
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .env("AT_SPI_BUS_ADDRESS", &a11y)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("at-spi2-registryd is installed");
        *self.registry.lock().unwrap() = Some(Process(registry));
        wait_for_registry(&a11y).await;
        a11y
    }

    pub fn launch(&self, program: &str, args: &[&str], extra_env: &[(&str, &str)]) -> Process {
        let display = std::env::var("WAYLAND_DISPLAY")
            .expect("run inside a nested crownpositor: WAYLAND_DISPLAY is not set");
        let mut command = Command::new(program);
        command
            .args(args)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .env("WAYLAND_DISPLAY", display)
            .env_remove("WORKSPACE")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (name, value) in extra_env {
            command.env(name, value);
        }
        Process(command.spawn().expect("the probe starts"))
    }
}

pub async fn a11y_connection(address: &str) -> zbus::Connection {
    let address = address.try_into().unwrap();
    atspi::AccessibilityConnection::from_address(address)
        .await
        .unwrap()
        .connection()
        .clone()
}

/// The unique name of the application on the a11y bus that runs as `pid`.
pub async fn application_of(connection: &zbus::Connection, pid: u32) -> String {
    let dbus = zbus::fdo::DBusProxy::new(connection).await.unwrap();
    for _ in 0..100 {
        let names = dbus.list_names().await.unwrap();
        for name in names {
            if !name.starts_with(':') {
                continue;
            }
            let owner = dbus
                .get_connection_unix_process_id(name.as_ref())
                .await
                .ok();
            if owner == Some(pid) {
                return name.to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no application of pid {pid} joined the a11y bus");
}

async fn wait_for_registry(a11y: &str) {
    let connection = zbus::connection::Builder::address(a11y)
        .unwrap()
        .build()
        .await
        .unwrap();
    let dbus = zbus::fdo::DBusProxy::new(&connection).await.unwrap();
    for _ in 0..50 {
        let registry = zbus::names::BusName::try_from("org.a11y.atspi.Registry").unwrap();
        if dbus.name_has_owner(registry).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the a11y registry did not start");
}
