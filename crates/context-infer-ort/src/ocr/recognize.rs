use std::collections::BTreeMap;

use context_infer::{Image, InferError, TextBounds, TextLine};

use super::ctc::Labels;
use super::lines::{LinePiece, merge_overlapping_texts, split_into_pieces};
use super::tensor::{BgrPlanes, run_single, sample_bilinear};
use crate::session::GraphSession;

const DEFAULT_LINE_HEIGHT: usize = 48;
/// Line widths with a fixed batch each, so every recogniser shape is static and compiles
/// once on NPUs and TensorRT. A wider line is cut into pieces of the widest bucket.
pub(super) const WIDTH_BUCKETS: [(usize, usize); 3] = [(320, 32), (640, 16), (960, 8)];
pub(super) const WIDEST_BUCKET: usize = 960;
const MIN_CONFIDENCE: f32 = 0.5;

#[derive(Debug)]
pub(super) struct TextRecognizer {
    graph: GraphSession,
    line_height: usize,
    labels: Labels,
}

/// The text read from one piece of a box.
#[derive(Clone, Debug, Default)]
struct PieceReading {
    text: String,
    confidence: f32,
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
        let pieces: Vec<LinePiece> = boxes
            .iter()
            .enumerate()
            .flat_map(|(index, &bounds)| split_into_pieces(index, bounds, self.line_height))
            .collect();
        let mut by_bucket: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for (piece_index, piece) in pieces.iter().enumerate() {
            by_bucket.entry(piece.bucket).or_default().push(piece_index);
        }
        let mut readings = vec![PieceReading::default(); pieces.len()];
        for ((bucket, batch_size), members) in by_bucket {
            for batch in members.chunks(batch_size) {
                self.recognize_batch(image, &pieces, bucket, batch_size, batch, &mut readings)?;
            }
        }
        Ok(join_pieces(boxes, &pieces, &readings))
    }

    fn recognize_batch(
        &self,
        image: &Image,
        pieces: &[LinePiece],
        bucket: usize,
        batch_size: usize,
        batch: &[usize],
        readings: &mut [PieceReading],
    ) -> Result<(), InferError> {
        let plane = self.line_height * bucket;
        let mut values = vec![0f32; batch_size * 3 * plane];
        let mut rest = values.as_mut_slice();
        for piece in batch.iter().filter_map(|&index| pieces.get(index)) {
            let Some((mut planes, remaining)) = BgrPlanes::split(rest, plane) else {
                break;
            };
            fill_line(image, piece, self.line_height, bucket, &mut planes);
            rest = remaining;
        }
        let shape = [batch_size, 3, self.line_height, bucket];
        let input = self.graph.first_input()?;
        let output = self.graph.first_output()?;
        let (output_shape, probabilities) =
            run_single(&mut self.graph.lock(), input, output, shape, &values)?;
        let classes = output_shape
            .last()
            .and_then(|&classes| usize::try_from(classes).ok())
            .unwrap_or(0);
        if classes != self.labels.len() {
            return Err(InferError::Backend(format!(
                "the recogniser has {classes} classes, the dictionary {}",
                self.labels.len()
            )));
        }
        let per_line = probabilities.len() / batch_size;
        if per_line == 0 {
            return Err(InferError::Backend("the recogniser gave no output".into()));
        }
        for (&index, line_probabilities) in batch.iter().zip(probabilities.chunks(per_line)) {
            let (text, confidence) = self.labels.decode(line_probabilities, classes);
            if let Some(reading) = readings.get_mut(index) {
                *reading = PieceReading { text, confidence };
            }
        }
        Ok(())
    }
}

/// Puts the pieces of each box back together; drops boxes that read poorly.
fn join_pieces(
    boxes: &[TextBounds],
    pieces: &[LinePiece],
    readings: &[PieceReading],
) -> Vec<TextLine> {
    let mut lines = Vec::new();
    for (box_index, &bounds) in boxes.iter().enumerate() {
        let box_readings: Vec<&PieceReading> = pieces
            .iter()
            .zip(readings)
            .filter(|(piece, _)| piece.box_index == box_index)
            .map(|(_, reading)| reading)
            .collect();
        let texts: Vec<&str> = box_readings
            .iter()
            .map(|reading| reading.text.as_str())
            .collect();
        let text = merge_overlapping_texts(&texts);
        let confidence = box_readings
            .iter()
            .map(|reading| reading.confidence)
            .fold(f32::INFINITY, f32::min);
        if confidence >= MIN_CONFIDENCE && confidence.is_finite() && !text.trim().is_empty() {
            lines.push(TextLine {
                text,
                confidence,
                bounds,
            });
        }
    }
    lines
}

/// Scales the piece to the line height and its width, then leaves the rest of `bucket` as
/// padding.
#[allow(clippy::cast_precision_loss)]
pub(super) fn fill_line(
    image: &Image,
    piece: &LinePiece,
    line_height: usize,
    bucket: usize,
    planes: &mut BgrPlanes<'_>,
) {
    let bounds = piece.bounds;
    let x_scale = bounds.width as f32 / piece.width as f32;
    let y_scale = bounds.height as f32 / line_height as f32;
    for row in 0..line_height {
        let source_y = bounds.y as f32 + (row as f32 + 0.5) * y_scale - 0.5;
        let pixels = (0..piece.width).map(|column| {
            let source_x = bounds.x as f32 + (column as f32 + 0.5) * x_scale - 0.5;
            sample_bilinear(image, source_x, source_y)
        });
        planes.write_row(row, bucket, pixels, |value, _| {
            f32::from(value) / 127.5 - 1.0
        });
    }
}
