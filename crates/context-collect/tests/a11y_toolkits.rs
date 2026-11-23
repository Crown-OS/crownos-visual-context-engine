//! Reads real GTK 4 and Qt windows over a private session and a11y bus. The probes need a nested
//! crownpositor in a private, short runtime directory: in the user's own directory it replaces
//! the live compositor's control socket `crownos/crownpositor.sock`. The probes refuse any
//! display outside `/tmp`.
//!
//! ```sh
//! runtime=/tmp/vcge-nested; mkdir -p $runtime
//! XDG_RUNTIME_DIR=$runtime WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY \
//!     CROWN_CONFIG_DIR=$(mktemp -d) target/debug/compositor &   # save $! and kill only that
//! WAYLAND_DISPLAY=$runtime/wayland-1 \
//!     cargo test -p context-collect --test a11y_toolkits -- --ignored --test-threads=1
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
    AccessibilityBus, AccessibilityCollector, AccessibilityHost, AccessibleText, AtspiCall,
    FetchStrategy, FrameContent, RecentReadings, Toolkit, WindowReading, read_active_window,
};
use context_core::{Observation, ObservationKind, TextSource, WindowIdent, WindowRecord};
use tokio::sync::watch;

use support::{
    BODY_TEXT, GTK4_PROBE, GTK4_TITLE, LABEL_TEXT, PrivateSession, Process, QT_PROBE, QT_TITLE,
    RETYPED_TEXT, a11y_connection, application_of,
};

/// Generous: the probe may still be building its tree.
const TEST_BUDGET: Duration = Duration::from_secs(2);

async fn read_until_titled(
    connection: &zbus::Connection,
    bus_name: &str,
    title: &str,
    toolkit: Option<Toolkit>,
) -> WindowReading {
    for _ in 0..50 {
        if let Ok(reading) =
            read_active_window(connection.clone(), bus_name, TEST_BUDGET, toolkit).await
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
        other => panic!("expected text, got {other:?}"),
    }
}

fn assert_probe_text(lines: &[String]) {
    assert!(lines.iter().any(|line| line == LABEL_TEXT), "{lines:?}");
    assert!(lines.iter().any(|line| line == BODY_TEXT), "{lines:?}");
}

fn launch_gtk4(session: &PrivateSession, password: bool, retyped: &str) -> Process {
    let password = if password { "1" } else { "0" };
    session.launch(
        "python3",
        &[
            "-c", GTK4_PROBE, GTK4_TITLE, LABEL_TEXT, BODY_TEXT, password, retyped,
        ],
        &[("GDK_BACKEND", "wayland")],
    )
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
async fn gtk4_is_walked_once_then_read_from_its_cache() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = launch_gtk4(&session, false, "");
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let first = read_until_titled(&connection, &bus_name, GTK4_TITLE, None).await;
    assert_eq!(first.toolkit, Toolkit::Gtk4);
    assert_probe_text(text_of(&first).0);
    let second = read_active_window(connection, &bus_name, TEST_BUDGET, None)
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
    let probe = launch_gtk4(&session, true, "");
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let reading = read_until_titled(&connection, &bus_name, GTK4_TITLE, None).await;
    assert_eq!(reading.content, FrameContent::Sensitive);
    assert!(!reading.calls.contains(&AtspiCall::WholeText));
}

/// A walk read as if the window were Chromium's: every call it makes must be on Chromium's
/// allowlist.
#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
async fn a_chromium_walk_makes_only_allowed_calls() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = launch_gtk4(&session, false, "");
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let reading =
        read_until_titled(&connection, &bus_name, GTK4_TITLE, Some(Toolkit::Chromium)).await;
    let (lines, strategy) = text_of(&reading);
    assert_eq!(strategy, FetchStrategy::BoundedWalk);
    assert_probe_text(lines);
    assert!(reading.calls.contains(&AtspiCall::WholeText));
    for call in &reading.calls {
        assert!(call.permitted(Toolkit::Chromium), "{call:?}");
        assert!(!AtspiCall::MODE_RAISING.contains(&call.wire()), "{call:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and PySide6"]
async fn qt_is_read_through_collection_matches() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = session.launch(
        "python3",
        &["-c", QT_PROBE, QT_TITLE, LABEL_TEXT, BODY_TEXT],
        &[("QT_QPA_PLATFORM", "wayland")],
    );
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    let reading = read_until_titled(&connection, &bus_name, QT_TITLE, None).await;
    assert_eq!(reading.toolkit, Toolkit::Qt);
    let (lines, strategy) = text_of(&reading);
    assert_eq!(strategy, FetchStrategy::CollectionMatches);
    assert_probe_text(lines);
}

/// The compositor side, faked: one focused window that may be read.
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

    fn may_read(&self, _: &WindowRecord, _: &str) -> bool {
        true
    }

    fn flag_sensitive(&self, _: &WindowIdent) {}
}

fn focused_probe(pid: u32) -> (WindowIdent, FakeHost) {
    let ident = WindowIdent("probe".into());
    let mut windows = WindowTable::default();
    windows.upsert(WindowRecord {
        ident: ident.clone(),
        app_id: "org.crownos.VcgeProbeGtk4".into(),
        title: GTK4_TITLE.into(),
        pid: i32::try_from(pid).unwrap(),
        ..WindowRecord::default()
    });
    windows.set_focus(Some(ident.clone()));
    let focus = watch::Sender::new(Some(ident.clone()));
    (ident, FakeHost { windows, focus })
}

async fn next_text(
    observations: mpsc::Receiver<Observation>,
) -> (mpsc::Receiver<Observation>, WindowIdent, Vec<String>) {
    let (observations, observation) = tokio::task::spawn_blocking(move || {
        let observation = observations.recv_timeout(Duration::from_secs(10));
        (observations, observation)
    })
    .await
    .unwrap();
    let observation = observation.expect("the collector read the focused window");
    let ObservationKind::Text {
        window,
        source,
        lines,
        ..
    } = observation.kind
    else {
        panic!("expected text, got {:?}", observation.kind);
    };
    assert_eq!(source, TextSource::Accessibility);
    (observations, window, lines)
}

/// The first read comes from the compositor's focus; the second only from the AT-SPI text
/// change, through the registry, the match rules and the signal parser.
#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a nested crownpositor, dbus-daemon, at-spi2-core and python-gobject with GTK 4"]
async fn the_collector_reads_on_focus_and_again_after_typing() {
    let session = PrivateSession::start();
    let a11y = session.enable_accessibility().await;
    let probe = launch_gtk4(&session, false, RETYPED_TEXT);
    let connection = a11y_connection(&a11y).await;
    let bus_name = application_of(&connection, probe.pid()).await;
    read_until_titled(&connection, &bus_name, GTK4_TITLE, None).await;
    let (ident, host) = focused_probe(probe.pid());
    let (outlet, observations) = mpsc::sync_channel(16);
    let readings = RecentReadings::default();
    let _collector = AccessibilityCollector::spawn(
        outlet,
        host,
        AccessibilityBus::Address(a11y),
        readings.clone(),
    )
    .unwrap();
    let (observations, window, lines) = next_text(observations).await;
    assert_eq!(window, ident);
    assert_probe_text(&lines);
    let kept = readings.fresh(&ident, Duration::from_secs(5), Duration::ZERO);
    assert!(
        matches!(&kept, Some(AccessibleText::Lines(kept)) if *kept == lines),
        "{kept:?}"
    );
    let (_, window, lines) = next_text(observations).await;
    assert_eq!(window, ident);
    assert!(lines.iter().any(|line| line == RETYPED_TEXT), "{lines:?}");
}
