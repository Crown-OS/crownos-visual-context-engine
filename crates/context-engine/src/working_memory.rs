//! What is going on right now, as the API reports it.

use std::collections::VecDeque;

use context_core::{AppRef, Episode, Segment, Timestamp, WindowIdent};
use context_protocol::{ActivityView, CurrentContext, EpisodeGuess, FocusView};
use serde::{Deserialize, Serialize};

const RECENT_DOCUMENTS: usize = 64;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WorkingMemory {
    pub focus: Option<WindowIdent>,
    pub focus_app: Option<AppRef>,
    pub focus_title: String,
    pub segment: Option<Segment>,
    pub episode: Option<Episode>,
    recent_documents: VecDeque<String>,
    pub open_documents: u32,
}

impl WorkingMemory {
    pub fn note_document(&mut self, doc_key: &str) {
        if self
            .recent_documents
            .front()
            .is_some_and(|front| front == doc_key)
        {
            return;
        }
        self.recent_documents.retain(|known| known != doc_key);
        self.recent_documents.push_front(doc_key.to_owned());
        self.recent_documents.truncate(RECENT_DOCUMENTS);
    }

    pub fn recent_documents(&self) -> impl Iterator<Item = &str> {
        self.recent_documents.iter().map(String::as_str)
    }

    pub fn current_context(&self, now: Timestamp, observing: bool) -> CurrentContext {
        let focus = self.focus_app.as_ref().map(|app| FocusView {
            app: app.clone(),
            title: self.focus_title.clone(),
            doc_key: self
                .segment
                .as_ref()
                .map(|segment| segment.doc_key.to_string())
                .unwrap_or_default(),
            project: None,
        });
        let activity = self.segment.as_ref().map(|segment| ActivityView {
            label: segment.label,
            confidence: segment.label_confidence,
            since: segment.range.start,
        });
        let episode = self.episode.as_ref().map(|episode| EpisodeGuess {
            id: episode.id,
            title: episode.title.clone(),
            confidence: episode.confidence,
        });
        CurrentContext {
            t: now,
            focus,
            activity,
            episode,
            recent_documents: self.recent_documents.iter().take(10).cloned().collect(),
            open_documents: self.open_documents,
            observing,
        }
    }

    /// # Errors
    ///
    /// Fails only when a float is not finite.
    pub fn snapshot(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// # Errors
    ///
    /// Fails when the snapshot is from another version.
    pub fn restore(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_documents_are_unique_and_most_recent_first() {
        let mut memory = WorkingMemory::default();
        memory.note_document("a");
        memory.note_document("b");
        memory.note_document("a");
        memory.note_document("a");
        assert_eq!(
            memory.recent_documents().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        for i in 0..100 {
            memory.note_document(&i.to_string());
        }
        assert_eq!(memory.recent_documents().count(), RECENT_DOCUMENTS);
    }
}
