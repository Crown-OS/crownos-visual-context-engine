use std::collections::BTreeMap;

use context_infer::{Image, InferError, TextBounds, TextLine};

use super::ctc::Labels;
use super::tensor::{BgrPlanes, run_single, sample_bilinear};
use crate::session::GraphSession;

const DEFAULT_LINE_HEIGHT: usize = 48;
/// Fixed line widths, so each width compiles once on static-shape devices.
const WIDTH_BUCKETS: [usize; 3] = [320, 640, 960];
const MAX_BATCH: usize = 32;
const MIN_CONFIDENCE: f32 = 0.5;

#[derive(Debug)]
pub(super) struct TextRecognizer {
    graph: GraphSession,
    line_height: usize,
    labels: Labels,
}

impl TextRecognizer {
    pub(super) fn new(
        graph: GraphSession,
        shapes: &BTreeMap<String, Vec<i64>>,
        dictionary: &str,
    ) -> Self {
        let line_height = shapes
            .values()
            .next()
            .and_then(|shape| shape.get(2))
            .and_then(|&height| usize::try_from(height).ok())
            .filter(|&height| height > 0)
            .unwrap_or(DEFAULT_LINE_HEIGHT);
        Self {
            graph,
            line_height,
            labels: Labels::from_dictionary(dictionary),
        }
    }

    /// One line per box that reads with enough confidence, in the order of `boxes`.
    pub(super) fn recognize(
        &self,
        image: &Image,
        boxes: &[TextBounds],
    ) -> Result<Vec<TextLine>, InferError> {
        let mut by_bucket: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
        for (index, bounds) in boxes.iter().enumerate() {
            let width = self.scaled_width(bounds);
            let bucket = WIDTH_BUCKETS
                .into_iter()
                .find(|&bucket| bucket >= width)
                .unwrap_or(WIDTH_BUCKETS[2]);
            by_bucket
                .entry(bucket)
                .or_default()
                .push((index, width.min(bucket)));
        }
        let mut lines: Vec<Option<TextLine>> = vec![None; boxes.len()];
        for (bucket, members) in by_bucket {
            for batch in members.chunks(MAX_BATCH) {
                self.recognize_batch(image, boxes, bucket, batch, &mut lines)?;
            }
        }
        Ok(lines.into_iter().flatten().collect())
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn scaled_width(&self, bounds: &TextBounds) -> usize {
        let scale = self.line_height as f32 / bounds.height.max(1) as f32;
        ((bounds.width as f32 * scale).ceil() as usize).max(1)
    }

    fn recognize_batch(
        &self,
        image: &Image,
        boxes: &[TextBounds],
        bucket: usize,
        batch: &[(usize, usize)],
        lines: &mut [Option<TextLine>],
    ) -> Result<(), InferError> {
        let plane = self.line_height * bucket;
        let mut values = vec![0f32; batch.len() * 3 * plane];
        let mut rest = values.as_mut_slice();
        for &(index, width) in batch {
            let Some((mut planes, remaining)) = BgrPlanes::split(rest, plane) else {
                break;
            };
            if let Some(bounds) = boxes.get(index) {
                self.fill_line(image, bounds, width, bucket, &mut planes);
            }
            rest = remaining;
        }
        let shape = [batch.len(), 3, self.line_height, bucket];
        let input = self.graph.first_input()?;
        let output = self.graph.first_output()?;
        let (output_shape, probabilities) =
            run_single(&mut self.graph.lock(), input, output, shape, &values)?;
        let classes = output_shape
            .last()
            .and_then(|&classes| usize::try_from(classes).ok())
            .filter(|&classes| classes > 0)
            .ok_or_else(|| InferError::Backend("the recogniser output has no classes".into()))?;
        let per_line = probabilities.len() / batch.len().max(1);
        for (&(index, _), line_probabilities) in batch.iter().zip(probabilities.chunks(per_line)) {
            let (text, confidence) = self.labels.decode(line_probabilities, classes);
            let (Some(slot), Some(&bounds)) = (lines.get_mut(index), boxes.get(index)) else {
                continue;
            };
            if confidence >= MIN_CONFIDENCE && !text.trim().is_empty() {
                *slot = Some(TextLine {
                    text,
                    confidence,
                    bounds,
                });
            }
        }
        Ok(())
    }

    /// Scales the box to the line height and `width`, then pads it to `bucket`.
    #[allow(clippy::cast_precision_loss)]
    fn fill_line(
        &self,
        image: &Image,
        bounds: &TextBounds,
        width: usize,
        bucket: usize,
        planes: &mut BgrPlanes<'_>,
    ) {
        let x_scale = bounds.width as f32 / width as f32;
        let y_scale = bounds.height as f32 / self.line_height as f32;
        for row in 0..self.line_height {
            let source_y = bounds.y as f32 + (row as f32 + 0.5) * y_scale - 0.5;
            let pixels = (0..width).map(|column| {
                let source_x = bounds.x as f32 + (column as f32 + 0.5) * x_scale - 0.5;
                sample_bilinear(image, source_x, source_y)
            });
            planes.write_row(row, bucket, pixels, |value, _| {
                f32::from(value) / 127.5 - 1.0
            });
        }
    }
}
