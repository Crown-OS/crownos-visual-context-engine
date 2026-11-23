//! Regions that change in almost every capture, such as a clock, a spinner or a blinking caret,
//! learned per app. Their changes alone do not make a crop.

use std::num::NonZeroUsize;

use lru::LruCache;

use crate::tiles::{ChangedTiles, TileGrid};

/// A tile is volatile when it changed in at least this many of the last eight captures.
const VOLATILE_CHANGES_OF_EIGHT: u32 = 7;
const CAPTURES_BEFORE_LEARNING: u32 = 8;
/// When more of the window than this changes all the time, the window is busy (a video, a
/// scrolling log), not volatile, and nothing is masked.
const MAX_VOLATILE_PERCENT: usize = 5;
const REMEMBERED_APPS: NonZeroUsize = match NonZeroUsize::new(64) {
    Some(count) => count,
    None => NonZeroUsize::MIN,
};

#[derive(Debug)]
struct VolatileMask {
    grid: TileGrid,
    history: Vec<u8>,
    captures: u32,
}

impl VolatileMask {
    fn new(grid: TileGrid) -> Self {
        Self {
            grid,
            history: vec![0; grid.tile_count()],
            captures: 0,
        }
    }

    fn is_volatile(&self, index: usize) -> bool {
        self.captures >= CAPTURES_BEFORE_LEARNING
            && self
                .history
                .get(index)
                .is_some_and(|bits| bits.count_ones() >= VOLATILE_CHANGES_OF_EIGHT)
    }

    fn volatile_count(&self) -> usize {
        (0..self.history.len())
            .filter(|index| self.is_volatile(*index))
            .count()
    }

    fn record(&mut self, changed: &ChangedTiles) {
        for (index, bits) in self.history.iter_mut().enumerate() {
            *bits = (*bits << 1) | u8::from(changed.is_changed(index));
        }
        self.captures = self.captures.saturating_add(1);
    }
}

/// The learned masks of the apps seen most recently.
#[derive(Debug)]
pub(crate) struct VolatileMasks {
    by_app: LruCache<String, VolatileMask>,
}

impl Default for VolatileMasks {
    fn default() -> Self {
        Self {
            by_app: LruCache::new(REMEMBERED_APPS),
        }
    }
}

impl VolatileMasks {
    /// Removes the volatile tiles from `changed` with what was learned so far, then learns
    /// from this capture. A new window size starts the app's mask again.
    pub(crate) fn filter_and_learn(&mut self, app_key: &str, changed: &mut ChangedTiles) {
        let raw = changed.clone();
        let mask = match self.by_app.get_mut(app_key) {
            Some(mask) if mask.grid == changed.grid => mask,
            _ => {
                self.by_app
                    .put(app_key.to_owned(), VolatileMask::new(changed.grid));
                match self.by_app.get_mut(app_key) {
                    Some(mask) => mask,
                    None => return,
                }
            }
        };
        let volatile_limit = (changed.grid.tile_count() * MAX_VOLATILE_PERCENT / 100).max(1);
        if mask.volatile_count() <= volatile_limit {
            for index in raw.indices() {
                if mask.is_volatile(index) {
                    changed.set(index, false);
                }
            }
        }
        mask.record(&raw);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn changes(grid: TileGrid, indices: &[usize]) -> ChangedTiles {
        let mut changed = ChangedTiles::none(grid);
        for index in indices {
            changed.set(*index, true);
        }
        changed
    }

    #[test]
    fn a_clock_tile_is_masked_after_it_keeps_changing() {
        let grid = TileGrid::for_size(32 * 10, 32 * 10);
        let mut masks = VolatileMasks::default();
        for _ in 0..8 {
            let mut changed = changes(grid, &[9]);
            masks.filter_and_learn("clock.app", &mut changed);
            assert!(changed.is_changed(9));
        }
        let mut changed = changes(grid, &[9, 50]);
        masks.filter_and_learn("clock.app", &mut changed);
        assert_eq!(changed.indices().collect::<Vec<_>>(), [50]);
    }

    #[test]
    fn masks_are_kept_per_app() {
        let grid = TileGrid::for_size(320, 320);
        let mut masks = VolatileMasks::default();
        for _ in 0..9 {
            masks.filter_and_learn("clock.app", &mut changes(grid, &[9]));
        }
        let mut other = changes(grid, &[9]);
        masks.filter_and_learn("editor.app", &mut other);
        assert!(other.is_changed(9));
    }

    #[test]
    fn a_busy_window_is_not_masked() {
        let grid = TileGrid::for_size(320, 320);
        let everything: Vec<usize> = (0..grid.tile_count()).collect();
        let mut masks = VolatileMasks::default();
        for _ in 0..9 {
            masks.filter_and_learn("video.app", &mut changes(grid, &everything));
        }
        let mut changed = changes(grid, &everything);
        masks.filter_and_learn("video.app", &mut changed);
        assert_eq!(changed.count(), grid.tile_count());
    }
}
