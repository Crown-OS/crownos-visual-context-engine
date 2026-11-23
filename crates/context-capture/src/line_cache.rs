//! Text already read, by the hash of its line band, per window. A line that scrolled or came
//! back is not read again. The cache holds text and confidence only, never pixels.

use std::num::NonZeroUsize;

use context_core::WindowIdent;
use lru::LruCache;

const LINES_PER_WINDOW: NonZeroUsize = match NonZeroUsize::new(2_048) {
    Some(count) => count,
    None => NonZeroUsize::MIN,
};
const REMEMBERED_WINDOWS: NonZeroUsize = match NonZeroUsize::new(32) {
    Some(count) => count,
    None => NonZeroUsize::MIN,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CachedLine {
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug)]
pub(crate) struct LineCaches {
    by_window: LruCache<WindowIdent, LruCache<u64, CachedLine>>,
}

impl Default for LineCaches {
    fn default() -> Self {
        Self {
            by_window: LruCache::new(REMEMBERED_WINDOWS),
        }
    }
}

impl LineCaches {
    pub(crate) fn get(&mut self, window: &WindowIdent, band_hash: u64) -> Option<&CachedLine> {
        self.by_window.get_mut(window)?.get(&band_hash)
    }

    pub(crate) fn insert(&mut self, window: &WindowIdent, band_hash: u64, line: CachedLine) {
        self.by_window
            .get_or_insert_mut(window.clone(), || LruCache::new(LINES_PER_WINDOW))
            .put(band_hash, line);
    }

    /// Forgets all text of the window: it closed, or a secret was seen in it.
    pub(crate) fn forget_window(&mut self, window: &WindowIdent) {
        self.by_window.pop(window);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn line(text: &str) -> CachedLine {
        CachedLine {
            text: text.to_owned(),
            confidence: 0.9,
        }
    }

    #[test]
    fn lines_are_cached_per_window_and_forgotten_with_it() {
        let mut caches = LineCaches::default();
        let editor = WindowIdent("editor".into());
        let browser = WindowIdent("browser".into());
        caches.insert(&editor, 7, line("fn main() {"));
        assert_eq!(caches.get(&editor, 7).unwrap().text, "fn main() {");
        assert!(caches.get(&browser, 7).is_none());
        caches.forget_window(&editor);
        assert!(caches.get(&editor, 7).is_none());
    }

    #[test]
    fn the_least_recent_line_goes_first() {
        let mut caches = LineCaches::default();
        let window = WindowIdent("w".into());
        for hash in 0..=LINES_PER_WINDOW.get() as u64 {
            caches.insert(&window, hash, line("x"));
        }
        assert!(caches.get(&window, 0).is_none());
        assert!(caches.get(&window, 1).is_some());
    }
}
