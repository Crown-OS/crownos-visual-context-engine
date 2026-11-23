//! Reads real GTK 4 and Qt windows over a private a11y bus. Run inside a nested crownpositor:
//!
//! ```sh
//! WAYLAND_DISPLAY=<nested socket> cargo test -p context-collect --test a11y_toolkits -- --ignored
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::sync::mpsc;
use std::time::Duration;

use context_collect::WindowTable;
use context_collect::a11y::{
    AccessibilityBus, AccessibilityCollector, AccessibilityHost, FetchStrategy, FrameContent,
    Toolkit, WindowReading, read_active_window,
};
use context_core::{ObservationKind, TextSource, WindowIdent, WindowRecord};
use tokio::sync::watch;

use support::{
    BODY_TEXT, GTK4_PROBE, GTK4_TITLE, LABEL_TEXT, PrivateSession, QT_PROBE, QT_TITLE,
    a11y_connection, application_of,
};

/// Generous: the probe may still be building its tree.
const TEST_BUDGET: Duration = Duration::from_secs(2);

async fn read_until_titled(
    connection: &zbus::Connection,
    bus_name: &str,
    title: &str,
) -> WindowReading {
    for _ in 0..50 {
        if let Ok(reading) = read_active_window(connection, bus_name, TEST_BUDGET).await
            && reading.title == title
        {
            return reading;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("{bus_name} never showed a frame titled {title:?}");
}

fn text_of(reading: &WindowReading) -> (&[String], FetchStrategy) {
    match &reading.content {
        FrameContent::Text {
            lines, strategy, ..
        } => (lines, *strategy),
        FrameContent::PasswordField => panic!("no password field in this probe"),
    }
}

fn assert_probe_text(lines: &[String]) {
    assert!(lines.iter().any(|line| line == LABEL_TEXT), "{lines:?}");
    assert!(lines.iter().any(|line| line == BODY_TEXT), "{lines:?}");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
async fn gtk4_is_walked_once_then_read_from_its_cache() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = session.launch(
        "python3",
        &["-c", GTK4_PROBE, GTK4_TITLE, LABEL_TEXT, BODY_TEXT, "0"],
        &[("GDK_BACKEND", "wayland")],
    );
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let first = read_until_titled(&connection, &bus_name, GTK4_TITLE).await;
    assert_eq!(first.toolkit, Toolkit::Gtk4);
    assert_probe_text(text_of(&first).0);
    let second = read_active_window(&connection, &bus_name, TEST_BUDGET)
        .await
        .unwrap();
    let (lines, strategy) = text_of(&second);
    assert_eq!(
        strategy,
        FetchStrategy::CacheItems,
        "the walk filled the cache"
    );
    assert_probe_text(lines);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
async fn a_password_field_stops_the_read() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = session.launch(
        "python3",
        &["-c", GTK4_PROBE, GTK4_TITLE, LABEL_TEXT, BODY_TEXT, "1"],
        &[("GDK_BACKEND", "wayland")],
    );
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let reading = read_until_titled(&connection, &bus_name, GTK4_TITLE).await;
    assert_eq!(reading.content, FrameContent::PasswordField);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and PySide6"]
async fn qt_is_read() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = session.launch(
        "python3",
        &["-c", QT_PROBE, QT_TITLE, LABEL_TEXT, BODY_TEXT],
        &[("QT_QPA_PLATFORM", "wayland")],
    );
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let reading = read_until_titled(&connection, &bus_name, QT_TITLE).await;
    assert_eq!(reading.toolkit, Toolkit::Qt);
    let (lines, strategy) = text_of(&reading);
    eprintln!("Qt was read with {strategy:?}");
    assert_probe_text(lines);
}

/// The compositor side, faked: one window, focused, that may be read.
#[derive(Debug)]
struct FakeHost {
    windows: WindowTable,
    focus: watch::Sender<Option<WindowIdent>>,
}

impl AccessibilityHost for FakeHost {
    fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R {
        read(&self.windows)
    }

    fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>> {
        self.focus.subscribe()
    }

    fn may_read(&self, _: &WindowRecord) -> bool {
        true
    }

    fn flag_sensitive(&self, _: &WindowIdent) {}
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
async fn the_collector_sends_the_focused_window_text() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = session.launch(
        "python3",
        &["-c", GTK4_PROBE, GTK4_TITLE, LABEL_TEXT, BODY_TEXT, "0"],
        &[("GDK_BACKEND", "wayland")],
    );
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    read_until_titled(&connection, &bus_name, GTK4_TITLE).await;

    let ident = WindowIdent("probe".into());
    let mut windows = WindowTable::default();
    windows.upsert(WindowRecord {
        ident: ident.clone(),
        app_id: "org.crownos.VcgeProbeGtk4".into(),
        title: GTK4_TITLE.into(),
        pid: i32::try_from(probe.pid()).unwrap(),
        ..WindowRecord::default()
    });
    windows.set_focus(Some(ident.clone()));
    let host = FakeHost {
        windows,
        focus: watch::Sender::new(Some(ident.clone())),
    };
    let (outlet, observations) = mpsc::sync_channel(16);
    let _collector =
        AccessibilityCollector::spawn(outlet, host, AccessibilityBus::Address(a11y)).unwrap();
    let observation =
        tokio::task::spawn_blocking(move || observations.recv_timeout(Duration::from_secs(10)))
            .await
            .unwrap()
            .expect("the collector read the focused window");
    let ObservationKind::Text {
        window,
        source,
        lines,
        ..
    } = observation.kind
    else {
        panic!("expected text, got {:?}", observation.kind);
    };
    assert_eq!(window, ident);
    assert_eq!(source, TextSource::Accessibility);
    assert_probe_text(&lines);
}
