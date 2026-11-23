//! Where capture text and frame records go: the open segment of their window. A capture right
//! after a focus change comes before the segment opens, so its output waits here, in RAM and
//! briefly. If no segment opens for the window, the focus was a transition and the output goes.

use std::time::{Duration, Instant};

use context_core::{
    FrameRecord, Observation, ObservationKind, SegmentId, TextSource, Timestamp, WindowIdent,
};
use context_engine::Engine;
use context_store::{TextLine, Write};

const PENDING_TIME_TO_LIVE: Duration = Duration::from_secs(30);
const MAX_PENDING: usize = 16;

#[derive(Debug)]
enum Output {
    Text {
        source: TextSource,
        lines: Vec<String>,
    },
    Frame(FrameRecord),
}

#[derive(Debug)]
struct Pending {
    window: WindowIdent,
    t: Timestamp,
    output: Output,
    received: Instant,
}

impl Pending {
    fn into_write(self, segment: SegmentId) -> Write {
        match self.output {
            Output::Text { source, lines } => Write::TextLines {
                segment,
                lines: lines
                    .into_iter()
                    .map(|text| TextLine {
                        source,
                        text,
                        t: self.t,
                    })
                    .collect(),
            },
            Output::Frame(frame) => Write::Frame {
                segment,
                frame,
                t: self.t,
            },
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct CaptureOutput {
    pending: Vec<Pending>,
}

impl CaptureOutput {
    /// Takes the text or frame record of `observation`, if it has one.
    pub(crate) fn take(&mut self, observation: &Observation) {
        let (window, output) = match &observation.kind {
            ObservationKind::Text {
                window,
                source,
                lines,
                ..
            } => (
                window,
                Output::Text {
                    source: *source,
                    lines: lines.clone(),
                },
            ),
            ObservationKind::Frame(frame) => (&frame.window, Output::Frame(frame.clone())),
            _ => return,
        };
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push(Pending {
            window: window.clone(),
            t: observation.t,
            output,
            received: Instant::now(),
        });
    }

    /// The writes for output whose window has an open segment now. Output that expired, or
    /// whose window is no longer `allowed`, is dropped.
    pub(crate) fn ready_writes(
        &mut self,
        engine: &Engine,
        allowed: impl Fn(&WindowIdent) -> bool,
    ) -> Vec<Write> {
        let (ready, waiting): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut self.pending)
            .into_iter()
            .filter(|pending| {
                pending.received.elapsed() < PENDING_TIME_TO_LIVE && allowed(&pending.window)
            })
            .partition(|pending| engine.open_segment_of(&pending.window).is_some());
        self.pending = waiting;
        ready
            .into_iter()
            .filter_map(|pending| {
                let segment = engine.open_segment_of(&pending.window)?;
                Some(pending.into_write(segment))
            })
            .collect()
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }

    /// Drops the output of `window`, as when it showed a secret or closed.
    pub(crate) fn forget_window(&mut self, window: &WindowIdent) {
        self.pending.retain(|pending| pending.window != *window);
    }
}
