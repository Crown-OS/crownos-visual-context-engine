//! Segments that belong together become an episode: close in time, and about the same app or
//! the same place.

use context_core::{ActivityLabel, Episode, EpisodeId, EpisodeLevel, Micros, Segment, TimeRange};

const JOIN_GAP: Micros = Micros::from_secs(10 * 60);
const CLOSE_AFTER: Micros = Micros::from_secs(20 * 60);

#[derive(Debug, Clone, PartialEq)]
pub enum EpisodeEvent {
    Opened(Episode),
    Updated(Episode),
    Closed(Episode),
}

#[derive(Debug)]
pub struct EpisodeBuilder {
    next_id: u64,
    open: Option<Episode>,
    members: Vec<Segment>,
}

impl Default for EpisodeBuilder {
    fn default() -> Self {
        Self {
            next_id: 1,
            open: None,
            members: Vec::new(),
        }
    }
}

impl EpisodeBuilder {
    pub const fn with_next_id(mut self, next: u64) -> Self {
        self.next_id = next;
        self
    }

    pub const fn open_episode(&self) -> Option<&Episode> {
        self.open.as_ref()
    }

    /// A closed segment arrived; it joins the open episode or starts the next.
    pub fn add(&mut self, segment: &Segment) -> Vec<EpisodeEvent> {
        let mut events = Vec::new();
        let joins = self.open.as_ref().is_some_and(|open| {
            segment.range.start.since(open.range.end).0 < JOIN_GAP.0 && self.related(segment)
        });
        if !joins {
            events.extend(self.close());
            let episode = Episode {
                id: EpisodeId(self.next_id),
                level: EpisodeLevel::Task,
                parent: None,
                range: segment.range,
                title: title_for(std::slice::from_ref(segment)),
                summary: None,
                label: segment.label,
                project: segment.project,
                importance: segment.importance,
                confidence: 0.5,
                segments: vec![segment.id],
            };
            self.next_id += 1;
            self.members = vec![segment.clone()];
            self.open = Some(episode.clone());
            events.push(EpisodeEvent::Opened(episode));
            return events;
        }
        self.members.push(segment.clone());
        if let Some(open) = &mut self.open {
            open.range = TimeRange::new(open.range.start, segment.range.end.max(open.range.end));
            open.segments.push(segment.id);
            open.title = title_for(&self.members);
            open.label = dominant_label(&self.members);
            open.project = open.project.or(segment.project);
            open.importance = self
                .members
                .iter()
                .map(|member| member.importance)
                .fold(0.0, f32::max);
            open.confidence = (0.5 + 0.1 * self.members.len() as f32).min(0.9);
            events.push(EpisodeEvent::Updated(open.clone()));
        }
        events
    }

    /// Time passed with no segment: an episode left alone long enough closes.
    pub fn tick(&mut self, now: context_core::Timestamp) -> Vec<EpisodeEvent> {
        if self
            .open
            .as_ref()
            .is_some_and(|open| now.since(open.range.end).0 >= CLOSE_AFTER.0)
        {
            return self.close();
        }
        Vec::new()
    }

    pub fn close(&mut self) -> Vec<EpisodeEvent> {
        self.members.clear();
        self.open
            .take()
            .map(EpisodeEvent::Closed)
            .into_iter()
            .collect()
    }

    fn related(&self, segment: &Segment) -> bool {
        self.members.iter().any(|member| {
            member.app_key == segment.app_key
                || (member.project.is_some() && member.project == segment.project)
                || same_site(member.url.as_deref(), segment.url.as_deref())
                || same_parent(member.cwd.as_deref(), segment.cwd.as_deref())
        })
    }
}

fn same_site(a: Option<&str>, b: Option<&str>) -> bool {
    let host = |url: &str| {
        url.split("//")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .map(str::to_owned)
    };
    matches!((a.and_then(host), b.and_then(host)), (Some(x), Some(y)) if x == y)
}

fn same_parent(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

fn dominant_label(members: &[Segment]) -> ActivityLabel {
    let mut best = (ActivityLabel::Other, Micros::ZERO);
    for label in ActivityLabel::ALL {
        let active = members
            .iter()
            .filter(|member| member.label == label)
            .fold(Micros::ZERO, |sum, member| {
                sum.saturating_add(member.active)
            });
        if active.0 > best.1.0 {
            best = (label, active);
        }
    }
    best.0
}

/// The longest segment's title, with how many others joined it.
fn title_for(members: &[Segment]) -> String {
    let longest = members.iter().max_by_key(|member| member.active.0);
    match (longest, members.len()) {
        (Some(segment), 1) => segment.title.clone(),
        (Some(segment), count) => format!("{} (+{})", segment.title, count - 1),
        (None, _) => String::new(),
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
    use context_core::{DocKey, InputProfile, SegmentId, SegmentState, Timestamp};

    use super::*;

    fn segment(id: u64, app: &str, start: i64, end: i64) -> Segment {
        Segment {
            id: SegmentId(id),
            episode: None,
            app_key: app.into(),
            doc_key: DocKey::Title(format!("doc{id}")),
            title: format!("doc{id}"),
            url: None,
            file_path: None,
            cwd: None,
            project: None,
            range: TimeRange::new(Timestamp::from_secs(start), Timestamp::from_secs(end)),
            active: Micros::from_secs(end - start),
            input: InputProfile::default(),
            label: ActivityLabel::Coding,
            label_confidence: 0.9,
            importance: 0.4,
            text_coverage: 0.0,
            summary: None,
            state: SegmentState::Closed,
        }
    }

    #[test]
    fn nearby_related_segments_share_an_episode() {
        let mut builder = EpisodeBuilder::default();
        let first = builder.add(&segment(1, "code", 0, 600));
        assert!(matches!(first[0], EpisodeEvent::Opened(_)));
        let second = builder.add(&segment(2, "code", 700, 1_200));
        assert!(
            matches!(&second[0], EpisodeEvent::Updated(e) if e.segments.len() == 2 && e.title == "doc1 (+1)")
        );
        let far = builder.add(&segment(3, "code", 1_200 + 700, 3_000));
        assert!(matches!(far[0], EpisodeEvent::Closed(_)));
        assert!(matches!(far[1], EpisodeEvent::Opened(_)));
        let other_app = builder.add(&segment(4, "mail", 3_010, 3_100));
        assert!(matches!(other_app[0], EpisodeEvent::Closed(_)));
    }

    #[test]
    fn silence_closes_an_episode() {
        let mut builder = EpisodeBuilder::default();
        builder.add(&segment(1, "code", 0, 600));
        assert!(builder.tick(Timestamp::from_secs(600 + 1_199)).is_empty());
        assert!(matches!(
            builder.tick(Timestamp::from_secs(600 + 1_200))[0],
            EpisodeEvent::Closed(_)
        ));
    }
}
