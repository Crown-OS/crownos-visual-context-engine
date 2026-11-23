use context_infer::TextBounds;

/// A pixel with at least this probability (0.3) belongs to text.
const TEXT_THRESHOLD: u8 = 77;
/// A region keeps when its mean probability reaches 0.6.
const BOX_THRESHOLD: u8 = 153;
const MIN_SIDE: u32 = 3;
/// DB shrinks text regions in training; this grows them back.
const UNCLIP_RATIO: f32 = 1.5;

/// Axis-aligned boxes around the 4-connected text regions of a probability map.
pub(super) fn text_boxes(probability: &[u8], width: usize, height: usize) -> Vec<TextBounds> {
    let is_text = |index: usize| probability.get(index).is_some_and(|&p| p >= TEXT_THRESHOLD);
    let mut visited = vec![false; probability.len()];
    let mut stack = Vec::new();
    let mut boxes = Vec::new();
    for start in 0..probability.len() {
        if !is_text(start) || visited.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut region = Region::new(start % width, start / width);
        mark(&mut visited, start);
        stack.push(start);
        while let Some(index) = stack.pop() {
            let (x, y) = (index % width, index / width);
            region.add(x, y, probability.get(index).copied().unwrap_or(0));
            let neighbours = [
                (x > 0).then(|| index - 1),
                (x + 1 < width).then(|| index + 1),
                (y > 0).then(|| index - width),
                (y + 1 < height).then(|| index + width),
            ];
            for neighbour in neighbours.into_iter().flatten() {
                if is_text(neighbour) && !visited.get(neighbour).copied().unwrap_or(true) {
                    mark(&mut visited, neighbour);
                    stack.push(neighbour);
                }
            }
        }
        boxes.extend(region.into_bounds(width, height));
    }
    boxes
}

fn mark(visited: &mut [bool], index: usize) {
    if let Some(seen) = visited.get_mut(index) {
        *seen = true;
    }
}

struct Region {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
    probability_sum: u64,
    pixels: u64,
}

impl Region {
    const fn new(x: usize, y: usize) -> Self {
        Self {
            left: x,
            top: y,
            right: x,
            bottom: y,
            probability_sum: 0,
            pixels: 0,
        }
    }

    fn add(&mut self, x: usize, y: usize, probability: u8) {
        self.left = self.left.min(x);
        self.right = self.right.max(x);
        self.top = self.top.min(y);
        self.bottom = self.bottom.max(y);
        self.probability_sum += u64::from(probability);
        self.pixels += 1;
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn into_bounds(self, image_width: usize, image_height: usize) -> Option<TextBounds> {
        let width = (self.right - self.left + 1) as f32;
        let height = (self.bottom - self.top + 1) as f32;
        let mean = self.probability_sum / self.pixels.max(1);
        if mean < u64::from(BOX_THRESHOLD) || width.min(height) < MIN_SIDE as f32 {
            return None;
        }
        let grow = (width * height * UNCLIP_RATIO / (2.0 * (width + height))).round() as usize;
        let left = self.left.saturating_sub(grow);
        let top = self.top.saturating_sub(grow);
        let right = (self.right + grow).min(image_width.saturating_sub(1));
        let bottom = (self.bottom + grow).min(image_height.saturating_sub(1));
        Some(TextBounds {
            x: u32::try_from(left).ok()?,
            y: u32::try_from(top).ok()?,
            width: u32::try_from(right - left + 1).ok()?,
            height: u32::try_from(bottom - top + 1).ok()?,
        })
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_grows_each_confident_region() {
        let (width, height) = (40, 20);
        let mut probability = vec![0u8; width * height];
        for y in 5..9 {
            for x in 4..20 {
                probability[y * width + x] = 230;
            }
            for x in 28..32 {
                probability[y * width + x] = 100;
            }
        }
        let boxes = text_boxes(&probability, width, height);
        assert_eq!(
            boxes,
            [TextBounds {
                x: 2,
                y: 3,
                width: 20,
                height: 8
            }]
        );
    }
}
