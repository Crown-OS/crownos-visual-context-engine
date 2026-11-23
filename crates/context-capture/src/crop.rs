use crate::geometry::Rect;
use crate::tiles::ChangedTiles;

/// A text line cut by a tile edge is still read whole.
const LINE_MARGIN_VERTICAL: u32 = 12;
const LINE_MARGIN_HORIZONTAL: u32 = 16;

/// The one rectangle that holds every changed tile, with a line margin, inside the frame.
pub(crate) fn union_crop(changed: &ChangedTiles) -> Option<Rect> {
    let grid = changed.grid;
    let frame = Rect::new(0, 0, grid.width, grid.height);
    changed
        .indices()
        .map(|index| grid.tile_bounds(index))
        .reduce(Rect::union)
        .map(|union| union.expanded_within(LINE_MARGIN_HORIZONTAL, LINE_MARGIN_VERTICAL, frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileGrid;

    #[test]
    fn the_crop_spans_the_changed_tiles_plus_a_margin() {
        let grid = TileGrid::for_size(320, 200);
        let mut changed = ChangedTiles::none(grid);
        assert_eq!(union_crop(&changed), None);
        changed.set(11, true);
        changed.set(23, true);
        assert_eq!(union_crop(&changed), Some(Rect::new(16, 20, 128, 88)));
        changed.set(0, true);
        assert_eq!(union_crop(&changed), Some(Rect::new(0, 0, 144, 108)));
    }
}
