//! The text of a crop in two steps. While the frame exists, its line bands are found and
//! looked up in the line cache. After the frame is zeroed, the bands that were not cached are
//! read from their sealed copy by OCR in one call.

use std::time::Instant;

use context_core::WindowIdent;

use crate::frame::CapturedFrame;
use crate::geometry::Rect;
use crate::line_bands::{LineBand, find_line_bands};
use crate::line_cache::{CachedLine, LineCaches};
use crate::region_copy::RegionCopy;
use crate::screen_models::{RecognizedLine, ScreenModels};

const MIN_LINE_CONFIDENCE: f32 = 0.5;

/// What is known of a crop before OCR.
#[derive(Debug)]
pub(super) struct CropPlan {
    bands: Vec<LineBand>,
    placed: Vec<(Rect, String)>,
    unread: Vec<LineBand>,
}

/// The text of a crop. Lines left unread make the next capture of the window read it whole,
/// because the tile hashes already moved past this frame.
#[derive(Debug)]
pub(super) struct CropReading {
    pub(super) lines: Vec<String>,
    pub(super) lines_left_unread: bool,
}

impl CropPlan {
    pub(super) fn new(
        frame: &CapturedFrame,
        crop: Rect,
        window: &WindowIdent,
        caches: &mut LineCaches,
    ) -> Self {
        let bands = find_line_bands(frame, crop);
        let mut placed = Vec::new();
        let mut unread = Vec::new();
        for band in &bands {
            match caches.get(window, band.hash) {
                Some(cached) => placed.push((band.bounds, cached.text.clone())),
                None => unread.push(*band),
            }
        }
        Self {
            bands,
            placed,
            unread,
        }
    }

    /// The one rectangle OCR must read, if any band is not cached.
    pub(super) fn unread_span(&self) -> Option<Rect> {
        self.unread
            .iter()
            .map(|band| band.bounds)
            .reduce(Rect::union)
    }

    /// Cached lines as they were, and the others from `copy` through OCR. Without a copy (a
    /// cancel or a failed copy), the unread lines stay unread.
    pub(super) fn read(
        mut self,
        window: &WindowIdent,
        copy: Option<&RegionCopy>,
        models: &dyn ScreenModels,
        caches: &mut LineCaches,
        deadline: Instant,
    ) -> CropReading {
        let mut lines_left_unread = false;
        if !self.unread.is_empty() {
            let recognized = copy.and_then(|copy| {
                models
                    .recognize_text(copy, deadline)
                    .map_err(|error| tracing::debug!(%error, "no OCR for this capture"))
                    .ok()
            });
            match recognized {
                Some(recognized) => self.place(window, recognized, caches),
                None => lines_left_unread = true,
            }
        }
        self.placed.sort_by_key(|(bounds, _)| (bounds.y, bounds.x));
        CropReading {
            lines: self.placed.into_iter().map(|(_, text)| text).collect(),
            lines_left_unread,
        }
    }

    /// Gives each recognized line to the band its middle is in. Lines of unread bands are
    /// joined and cached; lines of cached bands are already placed; other lines stand alone.
    fn place(
        &mut self,
        window: &WindowIdent,
        recognized: Vec<RecognizedLine>,
        caches: &mut LineCaches,
    ) {
        let mut per_band: Vec<Vec<RecognizedLine>> = vec![Vec::new(); self.unread.len()];
        for line in recognized {
            if line.confidence < MIN_LINE_CONFIDENCE || line.text.trim().is_empty() {
                continue;
            }
            let middle = line.bounds.y + line.bounds.height / 2;
            let inside = |band: &LineBand| band.bounds.y <= middle && middle < band.bounds.bottom();
            if let Some(slot) = self
                .unread
                .iter()
                .position(inside)
                .and_then(|index| per_band.get_mut(index))
            {
                slot.push(line);
            } else if !self.bands.iter().any(inside) {
                self.placed.push((line.bounds, line.text));
            }
        }
        for (band, mut lines) in self.unread.iter().zip(per_band) {
            if lines.is_empty() {
                continue;
            }
            lines.sort_by_key(|line| line.bounds.x);
            let confidence =
                lines.iter().map(|line| line.confidence).sum::<f32>() / lines.len() as f32;
            let text = lines
                .into_iter()
                .map(|line| line.text)
                .collect::<Vec<_>>()
                .join(" ");
            caches.insert(
                window,
                band.hash,
                CachedLine {
                    text: text.clone(),
                    confidence,
                },
            );
            self.placed.push((band.bounds, text));
        }
    }
}
