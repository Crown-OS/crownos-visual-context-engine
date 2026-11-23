use context_infer::TextBounds;

use super::recognize::{WIDEST_BUCKET, WIDTH_BUCKETS};

/// A part of one text box that fits a width bucket at the line height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LinePiece {
    pub(super) box_index: usize,
    pub(super) bounds: TextBounds,
    /// The width after scaling to the line height.
    pub(super) width: usize,
    /// (width, batch size) of the bucket it goes in.
    pub(super) bucket: (usize, usize),
}

/// The box as one piece, or, when it is wider than the widest bucket at the line height, as
/// pieces of that width that overlap by one box height, so no character is cut in all of
/// them.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(super) fn split_into_pieces(
    box_index: usize,
    bounds: TextBounds,
    line_height: usize,
) -> Vec<LinePiece> {
    let scale = line_height as f32 / bounds.height.max(1) as f32;
    let scaled = |width: u32| ((width as f32 * scale).ceil() as usize).clamp(1, WIDEST_BUCKET);
    let piece = |bounds: TextBounds| {
        let width = scaled(bounds.width);
        let bucket = WIDTH_BUCKETS
            .into_iter()
            .find(|&(bucket_width, _)| bucket_width >= width)
            .unwrap_or((WIDEST_BUCKET, 8));
        LinePiece {
            box_index,
            bounds,
            width,
            bucket,
        }
    };
    let widest_source = ((WIDEST_BUCKET as f32 / scale).floor() as u32).max(1);
    if bounds.width <= widest_source {
        return vec![piece(bounds)];
    }
    let overlap = bounds.height.min(widest_source / 2);
    let step = (widest_source - overlap).max(1);
    let last_left = bounds.x + bounds.width - widest_source;
    let mut pieces = Vec::new();
    let mut left = bounds.x;
    loop {
        let piece_left = left.min(last_left);
        pieces.push(piece(TextBounds {
            x: piece_left,
            width: widest_source,
            ..bounds
        }));
        if piece_left == last_left {
            break;
        }
        left += step;
    }
    pieces
}

/// How many characters two neighbouring pieces of one box can both hold: a character is
/// about half the box height wide.
pub(super) fn shared_characters(previous: &LinePiece, next: &LinePiece) -> usize {
    let overlap = (previous.bounds.x + previous.bounds.width).saturating_sub(next.bounds.x);
    (2 * overlap).div_ceil(next.bounds.height.max(1)) as usize
}

/// Joins the texts of overlapping pieces. Each part names how many characters it can share
/// with the part before; the longest end of the text so far, up to that many characters,
/// that starts the part is kept once. The cap keeps repeated text such as "-----" whole.
pub(super) fn merge_overlapping_texts(parts: &[(&str, usize)]) -> String {
    let mut merged = String::new();
    for &(text, shared) in parts {
        let byte_after = |count: usize| {
            text.char_indices()
                .nth(count)
                .map_or(text.len(), |(index, _)| index)
        };
        let longest = shared.min(merged.chars().count()).min(text.chars().count());
        let overlap = (1..=longest)
            .rev()
            .find(|&count| {
                text.get(..byte_after(count))
                    .is_some_and(|prefix| merged.ends_with(prefix))
            })
            .unwrap_or(0);
        merged.push_str(text.get(byte_after(overlap)..).unwrap_or_default());
    }
    merged
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod tests {
    use context_infer::{Image, PixelFormat};

    use super::*;
    use crate::ocr::recognize::fill_line;
    use crate::ocr::tensor::BgrPlanes;

    const LINE_HEIGHT: usize = 48;

    #[test]
    fn a_narrow_box_is_one_piece_in_the_smallest_bucket() {
        let bounds = TextBounds {
            x: 5,
            y: 0,
            width: 100,
            height: 24,
        };
        let pieces = split_into_pieces(0, bounds, LINE_HEIGHT);
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0].width, pieces[0].bucket), (200, (320, 32)));
    }

    /// A 4000 x 20 line whose red channel counts columns in steps of 16: each piece must
    /// cover its own columns, fit the widest bucket, and overlap the next one.
    #[test]
    fn a_wide_rendered_line_is_cut_into_overlapping_pieces() {
        let (width, height) = (4000u32, 20u32);
        let pixels: Vec<u8> = (0..height)
            .flat_map(|_| (0..width).flat_map(|x| [(x / 16) as u8, 0, 0]))
            .collect();
        let image = Image::new(
            width,
            height,
            width as usize * 3,
            PixelFormat::Rgb8,
            pixels.into(),
        )
        .unwrap();
        let bounds = TextBounds {
            x: 0,
            y: 0,
            width,
            height,
        };
        let pieces = split_into_pieces(0, bounds, LINE_HEIGHT);
        assert!(pieces.len() > 1);
        assert_eq!(pieces[0].bounds.x, 0);
        let last = pieces.last().unwrap().bounds;
        assert_eq!(last.x + last.width, width);
        for pair in pieces.windows(2) {
            let end = pair[0].bounds.x + pair[0].bounds.width;
            assert!(pair[1].bounds.x + height <= end, "{pair:?}");
        }
        let plane = LINE_HEIGHT * WIDEST_BUCKET;
        for piece in &pieces {
            assert!(piece.width <= WIDEST_BUCKET);
            let mut values = vec![0f32; 3 * plane];
            let (mut planes, _) = BgrPlanes::split(&mut values, plane).unwrap();
            fill_line(&image, piece, LINE_HEIGHT, WIDEST_BUCKET, &mut planes);
            let red = |column: usize| ((values[2 * plane + column] + 1.0) * 127.5).round();
            let expected = |x: u32| f32::from((x / 16) as u8);
            assert!((red(0) - expected(piece.bounds.x)).abs() <= 1.0);
            let last_column = piece.bounds.x + piece.bounds.width - 1;
            assert!((red(piece.width - 1) - expected(last_column)).abs() <= 1.0);
        }
    }

    #[test]
    fn overlapping_texts_join_without_repeats() {
        assert_eq!(
            merge_overlapping_texts(&[("the quick br", 0), ("brown fox", 2)]),
            "the quick brown fox"
        );
        assert_eq!(merge_overlapping_texts(&[("ab", 0), ("cd", 2)]), "abcd");
        assert_eq!(merge_overlapping_texts(&[("ü€", 0), ("€x", 2)]), "ü€x");
    }

    #[test]
    fn repeated_text_loses_at_most_the_shared_characters() {
        assert_eq!(
            merge_overlapping_texts(&[("-----", 0), ("-----", 2)]),
            "--------"
        );
        assert_eq!(
            merge_overlapping_texts(&[("0000", 0), ("0000", 1)]),
            "0000000"
        );
    }

    #[test]
    fn a_character_cut_at_a_piece_edge_is_read_once() {
        assert_eq!(
            merge_overlapping_texts(&[("the quick bro", 0), ("own fox", 2)]),
            "the quick brown fox"
        );
        assert_eq!(
            merge_overlapping_texts(&[("the quick br", 0), ("own fox", 2)]),
            "the quick brown fox"
        );
    }

    #[test]
    fn pieces_one_height_apart_share_two_characters() {
        let bounds = TextBounds {
            x: 0,
            y: 0,
            width: 4000,
            height: 20,
        };
        let pieces = split_into_pieces(0, bounds, LINE_HEIGHT);
        assert_eq!(shared_characters(&pieces[0], &pieces[1]), 2);
    }
}
