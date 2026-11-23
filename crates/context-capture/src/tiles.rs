//! Which 32 px tiles changed. Only tiles inside the compositor's damage are hashed; a tile
//! outside it is unchanged by definition.

use xxhash_rust::xxh3::Xxh3;

use crate::frame::CapturedFrame;
use crate::geometry::Rect;
use crate::util::hash_seed::hash_seed;
use crate::util::row_buffer::RowBuffer;

pub(crate) const TILE_SIZE: u32 = 32;

/// The tile layout of a frame size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileGrid {
    pub width: u32,
    pub height: u32,
    pub columns: u32,
    pub rows: u32,
}

impl TileGrid {
    pub(crate) const fn for_size(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            columns: width.div_ceil(TILE_SIZE),
            rows: height.div_ceil(TILE_SIZE),
        }
    }

    pub(crate) const fn tile_count(self) -> usize {
        self.columns as usize * self.rows as usize
    }

    pub(crate) fn tile_bounds(self, index: usize) -> Rect {
        let column = u32::try_from(index % self.columns.max(1) as usize).unwrap_or(0);
        let row = u32::try_from(index / self.columns.max(1) as usize).unwrap_or(0);
        let x = column * TILE_SIZE;
        let y = row * TILE_SIZE;
        Rect::new(
            x,
            y,
            TILE_SIZE.min(self.width.saturating_sub(x)),
            TILE_SIZE.min(self.height.saturating_sub(y)),
        )
    }

    /// The tiles `rect` touches.
    pub(crate) fn tiles_in(self, rect: Rect) -> impl Iterator<Item = usize> {
        let first_column = rect.x / TILE_SIZE;
        let first_row = rect.y / TILE_SIZE;
        let last_column = rect.right().div_ceil(TILE_SIZE).min(self.columns);
        let last_row = rect.bottom().div_ceil(TILE_SIZE).min(self.rows);
        (first_row..last_row).flat_map(move |row| {
            (first_column..last_column).map(move |column| (row * self.columns + column) as usize)
        })
    }
}

/// The tiles that changed in one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChangedTiles {
    pub grid: TileGrid,
    changed: Vec<bool>,
}

impl ChangedTiles {
    pub(crate) fn none(grid: TileGrid) -> Self {
        Self {
            grid,
            changed: vec![false; grid.tile_count()],
        }
    }

    pub(crate) fn is_changed(&self, index: usize) -> bool {
        self.changed.get(index).copied().unwrap_or(false)
    }

    pub(crate) fn set(&mut self, index: usize, changed: bool) {
        if let Some(slot) = self.changed.get_mut(index) {
            *slot = changed;
        }
    }

    pub(crate) fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.changed
            .iter()
            .enumerate()
            .filter_map(|(index, changed)| changed.then_some(index))
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.changed.iter().filter(|changed| **changed).count()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        !self.changed.contains(&true)
    }
}

/// The tile hashes of the last frame of one window. Hashes only: no pixel is kept.
#[derive(Debug, Default)]
pub(crate) struct TileHashes {
    grid: Option<TileGrid>,
    hashes: Vec<u64>,
}

impl TileHashes {
    /// Hashes the damaged tiles of `frame` and compares them with the previous frame. A new
    /// size, or a first frame, hashes and reports every tile.
    pub(crate) fn update(&mut self, frame: &CapturedFrame) -> ChangedTiles {
        let grid = TileGrid::for_size(frame.width, frame.height);
        let mut changed = ChangedTiles::none(grid);
        let mut rows = RowBuffer::default();
        if self.grid != Some(grid) {
            self.grid = Some(grid);
            self.hashes = (0..grid.tile_count())
                .map(|index| hash_tile(frame, grid.tile_bounds(index), &mut rows))
                .collect();
            changed.changed.fill(true);
            return changed;
        }
        for rect in &frame.damage {
            for index in grid.tiles_in(*rect) {
                if changed.is_changed(index) {
                    continue;
                }
                let hash = hash_tile(frame, grid.tile_bounds(index), &mut rows);
                if let Some(previous) = self.hashes.get_mut(index)
                    && *previous != hash
                {
                    *previous = hash;
                    changed.set(index, true);
                }
            }
        }
        changed
    }
}

fn hash_tile(frame: &CapturedFrame, tile: Rect, rows: &mut RowBuffer) -> u64 {
    let mut hasher = Xxh3::with_seed(hash_seed());
    for y in tile.y..tile.bottom() {
        if let Some(row) = rows.read(frame, tile.x, y, tile.width) {
            hasher.update(row);
        }
    }
    hasher.digest()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_frames::{frame_with, paint};

    #[test]
    fn the_grid_covers_partial_tiles() {
        let grid = TileGrid::for_size(70, 33);
        assert_eq!((grid.columns, grid.rows), (3, 2));
        assert_eq!(grid.tile_bounds(5), Rect::new(64, 32, 6, 1));
        let touched: Vec<usize> = grid.tiles_in(Rect::new(31, 0, 2, 40)).collect();
        assert_eq!(touched, [0, 1, 3, 4]);
    }

    #[test]
    fn only_damaged_tiles_that_differ_are_changed() {
        let mut hashes = TileHashes::default();
        let first = frame_with(96, 64, |_| {}, vec![]);
        assert_eq!(hashes.update(&first).count(), 6);

        let painted_outside_damage = frame_with(96, 64, |pixels| paint(pixels, 96, 40, 40), vec![]);
        assert!(hashes.update(&painted_outside_damage).is_empty());

        let damage = vec![Rect::new(0, 0, 96, 64)];
        let painted = frame_with(96, 64, |pixels| paint(pixels, 96, 40, 40), damage.clone());
        let changed = hashes.update(&painted);
        assert_eq!(changed.indices().collect::<Vec<_>>(), [4]);

        let same = frame_with(96, 64, |pixels| paint(pixels, 96, 40, 40), damage);
        assert!(hashes.update(&same).is_empty());
    }
}
