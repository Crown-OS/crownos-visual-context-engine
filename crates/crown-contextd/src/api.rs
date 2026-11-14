//! The Context API served over crownos-ipc.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use context_collect::BridgeHandle;
use context_core::{Micros, Timestamp};
use context_protocol::context::{ContextChanged, Handler, SegmentClosed};
use context_protocol::{
    CurrentContext, EpisodeId, EpisodeView, Explanation, ForgetReport, ForgetRequest, Granularity,
    MemoryAssertion, MemoryKind, MemoryRef, Provenance, ResultKind, SearchHit, SearchQuery,
    SearchResults, SegmentView, SourceKind, Status, Timeline, TimelineQuery,
};
use context_store::{Store, Write, WriteHandle};
use crownos_ipc::{RemoteError, Server};

use crate::pipeline::Outgoing;
use crate::shared::Shared;

const MAX_RESULTS: u32 = 200;

pub(crate) struct Api {
    shared: Arc<Mutex<Shared>>,
    reader: Store,
    writes: WriteHandle,
    bridge: Option<BridgeHandle>,
}

impl Api {
    pub(crate) const fn new(
        shared: Arc<Mutex<Shared>>,
        reader: Store,
        writes: WriteHandle,
        bridge: Option<BridgeHandle>,
    ) -> Self {
        Self {
            shared,
            reader,
            writes,
            bridge,
        }
    }

    fn shared(&self) -> Result<std::sync::MutexGuard<'_, Shared>, RemoteError> {
        self.shared
            .lock()
            .map_err(|_| RemoteError::new(RemoteError::HANDLER, "engine state poisoned"))
    }

    fn audit(&self, action: &str, detail: String) {
        let _ = self.writes.send(Write::Audit {
            actor: "api".to_owned(),
            action: action.to_owned(),
            detail,
            t: Timestamp::now(),
        });
    }
}

fn store_error(error: &context_store::StoreError) -> RemoteError {
    RemoteError::new(RemoteError::HANDLER, error.to_string())
}

/// The local offset is not known to the daemon yet; days begin at UTC midnight.
const TZ_OFFSET: Micros = Micros::ZERO;

pub(crate) fn send_event(server: &mut Server, outgoing: Outgoing) {
    let sent = match outgoing {
        Outgoing::Context(context) => server.emit(&ContextChanged { context }),
        Outgoing::Segment(segment) => server.emit(&SegmentClosed { segment }),
    };
    if let Err(error) = sent {
        tracing::debug!(%error, "event not delivered");
    }
}

impl Handler for Api {
    fn status(&mut self) -> Result<Status, RemoteError> {
        let shared = self.shared()?;
        Ok(Status {
            state: shared.capture_state(),
            pause_reason: shared.pause_reason.clone(),
            compositor_connected: self.bridge.as_ref().is_some_and(BridgeHandle::is_connected),
            observations_seen: shared.observations_seen,
            observations_dropped: shared.observations_dropped,
            open_segments: u32::from(shared.engine.working_memory().segment.is_some()),
            pending_writes: 0,
        })
    }

    fn current_context(&mut self) -> Result<CurrentContext, RemoteError> {
        Ok(self.shared()?.engine.current_context(Timestamp::now()))
    }

    fn search(&mut self, query: SearchQuery) -> Result<SearchResults, RemoteError> {
        let started = Instant::now();
        let limit = query.limit.clamp(1, MAX_RESULTS);
        let range = query
            .time
            .map(|time| time.resolve(Timestamp::now(), TZ_OFFSET));
        let mut hits = Vec::new();
        let text_hits = self
            .reader
            .search_text(&query.text, limit)
            .map_err(|error| store_error(&error))?;
        let locator_hits = self
            .reader
            .search_locators(&query.text, limit)
            .map_err(|error| store_error(&error))?;
        let mut seen = std::collections::BTreeSet::new();
        let candidates = text_hits
            .iter()
            .map(|hit| {
                (
                    hit.segment_id,
                    Some(hit.snippet.clone()),
                    1.0 / (1.0 + hit.rank.abs()),
                )
            })
            .chain(locator_hits.iter().map(|id| (*id, None, 0.6)));
        for (segment_id, snippet, score) in candidates {
            if !seen.insert(segment_id) {
                continue;
            }
            let Some(row) = self
                .reader
                .segment(segment_id)
                .map_err(|error| store_error(&error))?
            else {
                continue;
            };
            if range.is_some_and(|range| !range.overlaps(&row.segment.range)) {
                continue;
            }
            if query
                .app_key
                .as_ref()
                .is_some_and(|key| *key != row.app_key)
            {
                continue;
            }
            let redacted = match query.redaction {
                context_protocol::Redaction::SummaryOnly => None,
                _ => snippet,
            };
            hits.push(SearchHit {
                kind: ResultKind::Segment,
                id: segment_id.0,
                app_key: row.app_key,
                title: row.segment.title.clone(),
                doc_key: row.segment.doc_key.to_string(),
                t_start: row.segment.range.start,
                episode: row.segment.episode,
                snippet: redacted,
                score,
                confidence: 0.95,
                provenance: vec![Provenance {
                    source: SourceKind::Compositor,
                    source_id: format!("segment:{segment_id}"),
                    t: row.segment.range.start,
                    snippet: None,
                }],
            });
            if u32::try_from(hits.len()).unwrap_or(u32::MAX) >= limit {
                break;
            }
        }
        self.audit("search", format!("{} hits", hits.len()));
        Ok(SearchResults {
            hits,
            took_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
        })
    }

    fn timeline(&mut self, query: TimelineQuery) -> Result<Timeline, RemoteError> {
        let range = query.time.resolve(Timestamp::now(), TZ_OFFSET);
        let segments: Vec<SegmentView> = self
            .reader
            .segments_between(range, query.app_key.as_deref(), query.min_importance)
            .map_err(|error| store_error(&error))?
            .iter()
            .map(|row| SegmentView::from(&row.segment))
            .collect();
        let episodes = match query.granularity {
            Granularity::Segment => Vec::new(),
            Granularity::Episode => self
                .reader
                .episodes_between(range)
                .map_err(|error| store_error(&error))?
                .iter()
                .map(|episode| {
                    let members = segments
                        .iter()
                        .filter(|segment| episode.segments.contains(&segment.id))
                        .cloned()
                        .collect();
                    EpisodeView::new(episode, members)
                })
                .collect(),
        };
        Ok(Timeline {
            range,
            day_summary: None,
            segments,
            episodes,
        })
    }

    fn episode(&mut self, id: EpisodeId) -> Result<Option<EpisodeView>, RemoteError> {
        let everything = context_core::TimeRange::new(Timestamp(0), Timestamp(i64::MAX));
        let Some(episode) = self
            .reader
            .episodes_between(everything)
            .map_err(|error| store_error(&error))?
            .into_iter()
            .find(|episode| episode.id == id)
        else {
            return Ok(None);
        };
        let mut members = Vec::new();
        for segment in &episode.segments {
            if let Some(row) = self
                .reader
                .segment(*segment)
                .map_err(|error| store_error(&error))?
            {
                members.push(SegmentView::from(&row.segment));
            }
        }
        Ok(Some(EpisodeView::new(&episode, members)))
    }

    fn explain(&mut self, memory: MemoryRef) -> Result<Explanation, RemoteError> {
        let evidence = match memory.kind {
            MemoryKind::Segment => self
                .reader
                .segment(context_core::SegmentId(memory.id))
                .map_err(|error| store_error(&error))?
                .map(|row| {
                    vec![Provenance {
                        source: SourceKind::Compositor,
                        source_id: format!("focus:{}", row.segment.range.start.0),
                        t: row.segment.range.start,
                        snippet: Some(row.segment.title),
                    }]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(Explanation {
            memory,
            evidence,
            derived_from: Vec::new(),
        })
    }

    fn remember(&mut self, assertion: MemoryAssertion) -> Result<MemoryRef, RemoteError> {
        let now = Timestamp::now();
        self.audit(
            "remember",
            format!(
                "{:?} {}={} by {:?}",
                assertion.kind, assertion.key, assertion.value, assertion.source
            ),
        );
        Ok(MemoryRef {
            kind: MemoryKind::Note,
            id: u64::try_from(now.0).unwrap_or(0),
        })
    }

    fn forget(&mut self, request: ForgetRequest) -> Result<ForgetReport, RemoteError> {
        let range = request.time.resolve(Timestamp::now(), TZ_OFFSET);
        let outcome = self
            .writes
            .forget(range, request.app_key.clone())
            .map_err(|error| store_error(&error))?;
        self.audit(
            "forget",
            format!("{range:?} {:?}: {outcome:?}", request.app_key),
        );
        Ok(ForgetReport {
            events: outcome.events,
            segments: outcome.segments,
            text_lines: outcome.text_lines,
            frames: outcome.frames,
            episodes_removed: outcome.episodes_removed,
        })
    }

    fn pause(&mut self, seconds: u64) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.paused = true;
            shared.pause_reason = Some(if seconds == 0 {
                "paused until resumed".to_owned()
            } else {
                format!("paused for {seconds} s")
            });
            let paused = context_core::Observation::new(
                Timestamp::now(),
                context_core::ObservationKind::Paused {
                    paused: true,
                    origin: context_core::PauseOrigin::Engine,
                },
            );
            shared.engine.ingest(&paused);
        }
        self.audit("pause", seconds.to_string());
    }

    fn resume(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.paused = false;
            shared.pause_reason = None;
            let resumed = context_core::Observation::new(
                Timestamp::now(),
                context_core::ObservationKind::Paused {
                    paused: false,
                    origin: context_core::PauseOrigin::Engine,
                },
            );
            shared.engine.ingest(&resumed);
        }
        self.audit("resume", String::new());
    }
}
