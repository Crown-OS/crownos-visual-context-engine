use std::collections::HashMap;
use std::time::Duration;

use context_core::WindowIdent;
use tokio::time::Instant;

use crate::util::simhash::{hamming_distance, simhash};

const REMEMBER_FOR: Duration = Duration::from_secs(60);
const NEAR_DUPLICATE_BITS: u32 = 10;
const FINGERPRINTS_PER_WINDOW: usize = 32;

/// Drops a window's text when text near enough to it was sent in the last minute.
#[derive(Debug, Default)]
pub(crate) struct TextDeduplicator {
    recent: HashMap<WindowIdent, Vec<(u64, Instant)>>,
}

impl TextDeduplicator {
    pub(crate) fn is_new(&mut self, window: &WindowIdent, lines: &[String], now: Instant) -> bool {
        self.recent.retain(|_, sent| {
            sent.retain(|(_, at)| now.duration_since(*at) < REMEMBER_FOR);
            !sent.is_empty()
        });
        let fingerprint = simhash(lines.iter().map(String::as_str));
        let sent = self.recent.entry(window.clone()).or_default();
        if sent
            .iter()
            .any(|(known, _)| hamming_distance(*known, fingerprint) <= NEAR_DUPLICATE_BITS)
        {
            return false;
        }
        if sent.len() >= FINGERPRINTS_PER_WINDOW {
            sent.remove(0);
        }
        sent.push((fingerprint, now));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split(". ").map(str::to_owned).collect()
    }

    #[test]
    fn near_text_in_the_same_window_is_dropped_for_a_minute() {
        let mut dedup = TextDeduplicator::default();
        let window = WindowIdent("editor".into());
        let now = Instant::now();
        let first = lines(
            "Meeting notes for the release. Ship the update daemon on Friday. \
            Check the rollback path in rEFInd before the tag",
        );
        let edited = lines(
            "Meeting notes for the release. Ship the update daemon on Monday. \
            Check the rollback path in rEFInd before the tag",
        );
        assert!(dedup.is_new(&window, &first, now));
        assert!(!dedup.is_new(&window, &edited, now + Duration::from_secs(5)));
        assert!(dedup.is_new(&WindowIdent("other".into()), &first, now));
        assert!(dedup.is_new(&window, &first, now + REMEMBER_FOR));
    }
}
