//! The shape of the day, as a list of tasks the generator plays out.

use context_core::{
    ActivityLabel, InputCounts, Micros, Observation, ObservationKind, Placement, SelectionKind,
    SessionState, TextSource, TimeRange, Timestamp, WindowIdent, WindowRecord, WindowState,
};

use crate::gold::GoldSegment;
use crate::trace::Trace;
use crate::util::rng::Rng;

/// 2026-11-02, a Monday, 09:00 UTC.
const DAY_START: Timestamp = Timestamp::from_secs(1_793_610_000);
const BUCKET: Micros = Micros::from_secs(10);
const LUNCH_IDLE: Micros = Micros::from_secs(40 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Nine hours: coding with research, a review, chat and email, lunch away, more coding.
    Workday,
    /// Ninety minutes of coding and browsing, for fast tests.
    Short,
}

#[derive(Debug, Clone)]
struct Document {
    title: String,
    url: Option<String>,
    doc_key: String,
}

#[derive(Debug, Clone)]
struct App {
    app_id: &'static str,
    window: &'static str,
    label: ActivityLabel,
    documents: Vec<Document>,
    pid: i32,
}

#[derive(Debug, Clone, Copy)]
struct Task {
    apps: &'static [usize],
    minutes: i64,
    typing: bool,
}

struct Generator {
    rng: Rng,
    now: Timestamp,
    observations: Vec<Observation>,
    gold: Vec<GoldSegment>,
    apps: Vec<App>,
    current_doc: Vec<usize>,
    focus: Option<usize>,
}

fn document(title: &str, url: Option<&str>, doc_key: &str) -> Document {
    Document {
        title: title.to_owned(),
        url: url.map(str::to_owned),
        doc_key: doc_key.to_owned(),
    }
}

fn apps() -> Vec<App> {
    vec![
        App {
            app_id: "code-oss",
            window: "code-1",
            label: ActivityLabel::Coding,
            pid: 4101,
            documents: vec![
                document(
                    "mod.rs — crownpositor",
                    None,
                    "file:/home/u/crownpositor/compositor/src/context/mod.rs",
                ),
                document(
                    "fanout.rs — crownpositor",
                    None,
                    "file:/home/u/crownpositor/compositor/src/context/fanout.rs",
                ),
                document(
                    "lib.rs — context-core",
                    None,
                    "file:/home/u/crownos-visual-context-engine/crates/context-core/src/lib.rs",
                ),
            ],
        },
        App {
            app_id: "firefox",
            window: "firefox-1",
            label: ActivityLabel::ResearchBrowsing,
            pid: 4202,
            documents: vec![
                document(
                    "Smithay damage tracking — docs.rs — Mozilla Firefox",
                    Some(
                        "https://docs.rs/smithay/latest/smithay/backend/renderer/damage/index.html",
                    ),
                    "url:https://docs.rs/smithay/latest/smithay/backend/renderer/damage/index.html",
                ),
                document(
                    "niri/src/screencasting/pw_utils.rs at main · YaLTeR/niri — Mozilla Firefox",
                    Some("https://github.com/YaLTeR/niri/blob/main/src/screencasting/pw_utils.rs"),
                    "url:https://github.com/YaLTeR/niri/blob/main/src/screencasting/pw_utils.rs",
                ),
                document(
                    "wayland-protocols ext-image-copy-capture-v1 — Mozilla Firefox",
                    Some("https://wayland.app/protocols/ext-image-copy-capture-v1"),
                    "url:https://wayland.app/protocols/ext-image-copy-capture-v1",
                ),
            ],
        },
        App {
            app_id: "kitty",
            window: "kitty-1",
            label: ActivityLabel::TerminalOps,
            pid: 4303,
            documents: vec![document(
                "~/crownpositor — fish",
                None,
                "dir:/home/u/crownpositor",
            )],
        },
        App {
            app_id: "org.mozilla.Thunderbird",
            window: "thunderbird-1",
            label: ActivityLabel::Email,
            pid: 4404,
            documents: vec![document(
                "Inbox (12) — Mozilla Thunderbird",
                None,
                "title:inbox mozilla thunderbird",
            )],
        },
        App {
            app_id: "com.slack.Slack",
            window: "slack-1",
            label: ActivityLabel::CommunicationChat,
            pid: 4505,
            documents: vec![document(
                "#compositor — CrownOS — Slack",
                None,
                "title:compositor crownos slack",
            )],
        },
    ]
}

const WORKDAY: &[Task] = &[
    Task {
        apps: &[3],
        minutes: 25,
        typing: true,
    },
    Task {
        apps: &[0, 1, 2],
        minutes: 95,
        typing: true,
    },
    Task {
        apps: &[4],
        minutes: 15,
        typing: true,
    },
    Task {
        apps: &[1, 0],
        minutes: 60,
        typing: false,
    },
    Task {
        apps: &[0, 2],
        minutes: 45,
        typing: true,
    },
    Task {
        apps: &[1],
        minutes: 20,
        typing: false,
    },
    Task {
        apps: &[0, 2, 1],
        minutes: 120,
        typing: true,
    },
    Task {
        apps: &[4, 3],
        minutes: 30,
        typing: true,
    },
];

const SHORT: &[Task] = &[
    Task {
        apps: &[0, 1, 2],
        minutes: 60,
        typing: true,
    },
    Task {
        apps: &[1],
        minutes: 30,
        typing: false,
    },
];

/// The trace for `scenario` and `seed`. Same inputs, same bytes.
pub fn generate(scenario: Scenario, seed: u64) -> Trace {
    let apps = apps();
    let mut generator = Generator {
        rng: Rng::seeded(seed),
        now: DAY_START,
        observations: Vec::new(),
        gold: Vec::new(),
        current_doc: vec![0; apps.len()],
        apps,
        focus: None,
    };
    generator.open_windows();
    let tasks = match scenario {
        Scenario::Workday => WORKDAY,
        Scenario::Short => SHORT,
    };
    let lunch_after = tasks.len() / 2;
    for (index, task) in tasks.iter().enumerate() {
        generator.play(*task);
        if index + 1 == lunch_after && scenario == Scenario::Workday {
            generator.lunch();
        }
    }
    generator.close_day();
    Trace {
        seed,
        observations: generator.observations,
        gold: generator.gold,
    }
}

impl Generator {
    fn emit(&mut self, kind: ObservationKind) {
        self.observations.push(Observation::new(self.now, kind));
    }

    const fn advance(&mut self, by: Micros) {
        self.now = self.now.plus(by);
    }

    fn record(&self, app: usize) -> Option<WindowRecord> {
        let application = self.apps.get(app)?;
        let document = application.documents.get(*self.current_doc.get(app)?)?;
        Some(WindowRecord {
            ident: WindowIdent(application.window.to_owned()),
            app_id: application.app_id.to_owned(),
            title: document.title.clone(),
            pid: application.pid,
            sandbox: None,
            placement: Some(Placement {
                x: 0,
                y: 0,
                width: 2560,
                height: 1400,
            }),
            output: "DP-1".to_owned(),
            workspace: "crownos-workspace-1".to_owned(),
            state: WindowState {
                visible: true,
                activated: self.focus == Some(app),
                ..WindowState::default()
            },
            ..WindowRecord::default()
        })
    }

    fn open_windows(&mut self) {
        self.emit(ObservationKind::Session(SessionState::Active));
        for app in 0..self.apps.len() {
            if let Some(record) = self.record(app) {
                self.emit(ObservationKind::WindowOpened(record));
            }
            self.advance(Micros::from_millis(350));
        }
    }

    fn window_ident(&self, app: usize) -> WindowIdent {
        WindowIdent(
            self.apps
                .get(app)
                .map(|a| a.window.to_owned())
                .unwrap_or_default(),
        )
    }

    fn focus(&mut self, app: usize) {
        self.focus = Some(app);
        let ident = self.window_ident(app);
        self.emit(ObservationKind::Focus(Some(ident)));
        if let Some(record) = self.record(app) {
            self.emit(ObservationKind::WindowChanged(record));
        }
    }

    fn switch_document(&mut self, app: usize) {
        let count = self.apps.get(app).map_or(0, |a| a.documents.len());
        if count < 2 {
            return;
        }
        let next = usize::try_from(self.rng.range(0, count as i64 - 1)).unwrap_or(0);
        if let Some(current) = self.current_doc.get_mut(app) {
            *current = next;
        }
        if let Some(record) = self.record(app) {
            self.emit(ObservationKind::WindowChanged(record));
        }
        let url = self
            .apps
            .get(app)
            .and_then(|a| a.documents.get(next))
            .and_then(|d| d.url.clone());
        if let Some(url) = url {
            let window = self.window_ident(app);
            self.emit(ObservationKind::Text {
                window,
                source: TextSource::Accessibility,
                url: Some(url),
                lines: Vec::new(),
            });
        }
    }

    /// Attention on one document for `dwell`, as ten-second input buckets.
    fn dwell(&mut self, app: usize, dwell: Micros, typing: bool) {
        let start = self.now;
        let window = self.window_ident(app);
        let mut elapsed = Micros::ZERO;
        while elapsed.0 < dwell.0 {
            let step = if dwell.0 - elapsed.0 < BUCKET.0 {
                Micros(dwell.0 - elapsed.0)
            } else {
                BUCKET
            };
            let counts = self.bucket(typing);
            if !counts.is_empty() {
                self.emit(ObservationKind::InputActivity {
                    window: Some(window.clone()),
                    interval: step,
                    counts,
                });
            }
            if typing && self.rng.chance(2) {
                let size_hint = u32::try_from(self.rng.range(12, 900)).unwrap_or(64);
                self.emit(ObservationKind::Selection {
                    kind: SelectionKind::Clipboard,
                    window: Some(window.clone()),
                    mime_types: vec!["text/plain;charset=utf-8".to_owned()],
                    size_hint,
                    secret: false,
                });
            }
            self.advance(step);
            elapsed = elapsed.saturating_add(step);
        }
        let Some(application) = self.apps.get(app) else {
            return;
        };
        let Some(document) = self
            .current_doc
            .get(app)
            .and_then(|index| application.documents.get(*index))
        else {
            return;
        };
        self.gold.push(GoldSegment {
            app_id: application.app_id.to_owned(),
            window: application.window.to_owned(),
            doc_key: document.doc_key.clone(),
            range: TimeRange::new(start, self.now),
            label: application.label,
        });
    }

    fn bucket(&mut self, typing: bool) -> InputCounts {
        let text_keys = if typing && self.rng.chance(80) {
            u32::try_from(self.rng.range(5, 60)).unwrap_or(0)
        } else {
            0
        };
        InputCounts {
            text_keys,
            edit_keys: if text_keys > 0 { text_keys / 8 } else { 0 },
            nav_keys: u32::try_from(self.rng.range(0, 6)).unwrap_or(0),
            chords: u32::try_from(self.rng.range(0, 2)).unwrap_or(0),
            clicks: u32::try_from(self.rng.range(0, 4)).unwrap_or(0),
            scroll_ticks: if typing {
                0
            } else {
                u32::try_from(self.rng.range(0, 40)).unwrap_or(0)
            },
            pointer_travel: u32::try_from(self.rng.range(0, 12) * 100).unwrap_or(0),
            typing_paused: text_keys == 0,
            scroll_stopped: false,
        }
    }

    /// A glance at another window, too short to be attention.
    fn flicker(&mut self, from: usize) {
        let Some(&other) = self.rng.pick(
            &(0..self.apps.len())
                .filter(|&a| a != from)
                .collect::<Vec<_>>(),
        ) else {
            return;
        };
        self.focus(other);
        let glance = Micros::from_millis(self.rng.range(300, 1_500));
        self.advance(glance);
        self.focus(from);
    }

    fn play(&mut self, task: Task) {
        let end = self.now.plus(Micros::from_secs(task.minutes * 60));
        while self.now.0 < end.0 {
            let Some(&app) = self.rng.pick(task.apps) else {
                return;
            };
            if self.rng.chance(35) {
                self.switch_document(app);
            }
            self.focus(app);
            let remaining = end.since(self.now);
            let wanted = Micros::from_secs(self.rng.range(120, 900));
            let dwell = if wanted.0 < remaining.0 {
                wanted
            } else {
                remaining
            };
            self.dwell(app, dwell, task.typing);
            if self.rng.chance(20) {
                self.flicker(app);
            }
        }
    }

    fn lunch(&mut self) {
        self.emit(ObservationKind::Session(SessionState::Idle));
        self.advance(Micros::from_secs(5 * 60));
        self.emit(ObservationKind::Session(SessionState::Locked));
        self.focus = None;
        self.emit(ObservationKind::Focus(None));
        self.advance(LUNCH_IDLE);
        self.emit(ObservationKind::Session(SessionState::Active));
    }

    fn close_day(&mut self) {
        for app in 0..self.apps.len() {
            let ident = self.window_ident(app);
            self.emit(ObservationKind::WindowClosed(ident));
        }
        self.emit(ObservationKind::Session(SessionState::Locked));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_reproduces_the_day() {
        let a = generate(Scenario::Short, 42);
        let b = generate(Scenario::Short, 42);
        assert_eq!(a, b);
        assert_ne!(a, generate(Scenario::Short, 43));
    }

    #[test]
    fn gold_segments_are_ordered_and_disjoint() {
        let trace = generate(Scenario::Workday, 1);
        assert!(trace.gold.len() > 20);
        for pair in trace.gold.windows(2) {
            assert!(pair[0].range.end.0 <= pair[1].range.start.0);
        }
        let total: i64 = trace.gold.iter().map(|g| g.range.duration().0).sum();
        let span = trace.observations.last().unwrap().t.since(DAY_START);
        assert!(total <= span.0);
        assert!(trace.observations.len() > 1_000);
    }

    #[test]
    fn traces_round_trip_through_json() {
        let trace = generate(Scenario::Short, 9);
        let json = trace.to_json().unwrap();
        assert_eq!(Trace::from_json(&json).unwrap(), trace);
    }
}
