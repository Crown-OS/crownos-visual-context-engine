//! One capture, analysed at once: changed tiles, a crop, text from the window or from OCR, a
//! caption for frames with little text, and a secret check on everything. The result holds text
//! and numbers only.

mod crop_reading;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use context_core::{FrameRecord, Observation, ObservationKind, TextSource, Timestamp, WindowIdent};
use lru::LruCache;

use crop_reading::CropPlan;

use crate::cancellation::{Cancellation, ModelCall};
use crate::crop::union_crop;
use crate::frame::{CaptureTarget, CapturedFrame};
use crate::geometry::Rect;
use crate::image_regions::image_like_share;
use crate::line_cache::LineCaches;
use crate::region_copy::RegionCopy;
use crate::screen_models::{ScreenModels, ScreenSummary};
use crate::structured_text::{StructuredLines, StructuredReading, StructuredText};
use crate::tiles::TileHashes;
use crate::volatile_mask::VolatileMasks;

/// Below this, the frame has too little text to say what it shows.
const DESCRIBE_BELOW_TEXT_CHARACTERS: usize = 200;
const DESCRIBE_ABOVE_IMAGE_SHARE: f32 = 0.4;
/// A caption is for new content, not for a cursor that moved.
const DESCRIBE_FROM_DAMAGE_PERMILLE: u16 = 500;
/// All model calls of one capture finish within this, which also keeps an abort before sleep
/// inside logind's delay.
pub const CAPTURE_ANALYSIS_BUDGET: Duration = Duration::from_secs(4);
const REMEMBERED_WINDOWS: NonZeroUsize = match NonZeroUsize::new(32) {
    Some(count) => count,
    None => NonZeroUsize::MIN,
};

/// What the system is like at analysis time.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisConditions<'a> {
    pub on_battery: bool,
    pub cancel: &'a dyn Cancellation,
}

/// What one capture left behind: text and numbers, no pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureOutcome {
    pub target: CaptureTarget,
    pub captured_at: Timestamp,
    pub width: u32,
    pub height: u32,
    pub damage_permille: u16,
    pub text: Option<StructuredLines>,
    pub description: Option<ScreenSummary>,
    /// A secret was seen. The text is gone and the window must be flagged sensitive.
    pub secret_found: bool,
    /// The window showed a password field: the frame was dropped unread, and nothing of this
    /// capture is reported. The collector that saw the field flagged the window.
    pub withheld: bool,
    pub cancelled: bool,
}

impl CaptureOutcome {
    pub fn line_count(&self) -> usize {
        self.text.as_ref().map_or(0, |text| text.lines.len())
    }

    /// The OCR and caption text, then the frame record. Structured text is not repeated: its
    /// collector reported it already.
    pub fn observations(&self) -> Vec<Observation> {
        if self.withheld {
            return Vec::new();
        }
        let window = &self.target.window;
        let text = |source: TextSource, lines: Vec<String>| {
            Observation::new(
                self.captured_at,
                ObservationKind::Text {
                    window: window.clone(),
                    source,
                    url: None,
                    lines,
                },
            )
        };
        let mut observations = Vec::with_capacity(3);
        if let Some(found) = self
            .text
            .as_ref()
            .filter(|found| found.source == TextSource::Ocr && !found.lines.is_empty())
        {
            observations.push(text(found.source, found.lines.clone()));
        }
        if let Some(description) = &self.description {
            let lines = std::iter::once(description.caption.clone())
                .chain(description.visible_entities.iter().cloned())
                .filter(|line| !line.trim().is_empty())
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                observations.push(text(TextSource::Vision, lines));
            }
        }
        observations.push(Observation::new(
            self.captured_at,
            ObservationKind::Frame(FrameRecord {
                window: window.clone(),
                trigger: self.target.trigger,
                width: self.width,
                height: self.height,
                damage_permille: self.damage_permille,
                line_count: u32::try_from(self.line_count()).unwrap_or(u32::MAX),
                secret_seen: self.secret_found,
            }),
        ));
        observations
    }
}

pub struct FrameAnalyzer {
    models: Arc<dyn ScreenModels>,
    structured_text: Box<dyn StructuredText>,
    tile_hashes: LruCache<WindowIdent, TileHashes>,
    volatile_masks: VolatileMasks,
    line_caches: LineCaches,
}

impl std::fmt::Debug for FrameAnalyzer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FrameAnalyzer")
            .field("windows", &self.tile_hashes.len())
            .finish_non_exhaustive()
    }
}

impl FrameAnalyzer {
    pub fn new(models: Arc<dyn ScreenModels>, structured_text: Box<dyn StructuredText>) -> Self {
        Self {
            models,
            structured_text,
            tile_hashes: LruCache::new(REMEMBERED_WINDOWS),
            volatile_masks: VolatileMasks::default(),
            line_caches: LineCaches::default(),
        }
    }

    /// Analyses `frame`. Everything that needs its pixels runs first: tiles, bands, the line
    /// cache, and sealed copies of the regions the models read. Then the frame is zeroed and
    /// closed, before any model runs. A wait for structured text happens only with no pixel in
    /// memory.
    pub fn analyse(
        &mut self,
        frame: CapturedFrame,
        conditions: AnalysisConditions<'_>,
    ) -> CaptureOutcome {
        let deadline = Instant::now() + CAPTURE_ANALYSIS_BUDGET;
        let cancelled = || conditions.cancel.is_cancelled();
        let call = ModelCall {
            deadline,
            cancel: conditions.cancel,
        };
        let target = frame.target.clone();
        let window = &target.window;
        let structured = match self.structured_text.window_text(&target) {
            StructuredReading::Text(lines) => Some(lines),
            StructuredReading::Sensitive => return self.withhold(frame),
            StructuredReading::Pending => return self.read_without_pixels(frame, conditions),
            StructuredReading::Unavailable => None,
        };
        let first_capture = !self.tile_hashes.contains(window);
        let mut changed = self
            .tile_hashes
            .get_or_insert_mut(window.clone(), TileHashes::default)
            .update(&frame);
        self.volatile_masks
            .filter_and_learn(&target.app_key, &mut changed);
        let damage_permille = if first_capture {
            1000
        } else {
            frame.damage_permille()
        };

        let crop_plan = match structured {
            Some(_) => None,
            None => union_crop(&changed)
                .map(|crop| CropPlan::new(&frame, crop, window, &mut self.line_caches)),
        };
        let may_describe = !conditions.on_battery
            && damage_permille >= DESCRIBE_FROM_DAMAGE_PERMILLE
            && !cancelled();
        let image_like = may_describe && image_like_share(&frame) > DESCRIBE_ABOVE_IMAGE_SHARE;
        let ocr_copy = crop_plan
            .as_ref()
            .and_then(CropPlan::unread_span)
            .filter(|_| !cancelled())
            .and_then(|span| copy_region(&frame, span));
        let screen_copy = may_describe
            .then(|| copy_region(&frame, frame.bounds()))
            .flatten();
        let (captured_at, width, height) = (frame.captured_at, frame.width, frame.height);
        drop(frame);
        let text = match structured {
            Some(structured) => Some(structured),
            None => crop_plan.map(|plan| {
                let reading = plan.read(
                    window,
                    ocr_copy.as_ref(),
                    self.models.as_ref(),
                    &mut self.line_caches,
                    &call,
                );
                if reading.lines_left_unread {
                    self.tile_hashes.pop(window);
                }
                StructuredLines {
                    source: TextSource::Ocr,
                    lines: reading.lines,
                }
            }),
        };
        drop(ocr_copy);
        let text_characters: usize = text
            .iter()
            .flat_map(|text| &text.lines)
            .map(|line| line.chars().count())
            .sum();
        let description = screen_copy
            .filter(|_| {
                !cancelled() && (text_characters < DESCRIBE_BELOW_TEXT_CHARACTERS || image_like)
            })
            .and_then(|copy| {
                self.models
                    .describe_screen(&copy, &target.app_name, &target.title, &call)
                    .map_err(|error| tracing::debug!(%error, "no screen description"))
                    .ok()
            });

        let mut outcome = CaptureOutcome {
            target: target.clone(),
            captured_at,
            width,
            height,
            damage_permille,
            text,
            description,
            secret_found: false,
            withheld: false,
            cancelled: cancelled(),
        };
        if contains_secret(&outcome) {
            self.line_caches.forget_window(window);
            outcome.text = None;
            outcome.description = None;
            outcome.secret_found = true;
        }
        outcome
    }

    /// A reading of the window is on its way: the frame is zeroed before any of its pixels is
    /// read, then the analysis waits for the reading with no pixel in memory. Without a reading
    /// in time there is no OCR and no caption; the next capture covers the window.
    fn read_without_pixels(
        &mut self,
        frame: CapturedFrame,
        conditions: AnalysisConditions<'_>,
    ) -> CaptureOutcome {
        let (target, captured_at, width, height) = (
            frame.target.clone(),
            frame.captured_at,
            frame.width,
            frame.height,
        );
        drop(frame);
        let text = match self
            .structured_text
            .wait_for_text(&target, conditions.cancel)
        {
            StructuredReading::Text(lines) => Some(lines),
            StructuredReading::Sensitive => {
                self.forget_window(&target.window);
                return withheld(target, captured_at, width, height);
            }
            StructuredReading::Pending | StructuredReading::Unavailable => None,
        };
        let mut outcome = CaptureOutcome {
            target,
            captured_at,
            width,
            height,
            damage_permille: 0,
            text,
            description: None,
            secret_found: false,
            withheld: false,
            cancelled: conditions.cancel.is_cancelled(),
        };
        if contains_secret(&outcome) {
            outcome.text = None;
            outcome.secret_found = true;
        }
        outcome
    }

    /// Drops `frame` before any of its pixels is read.
    fn withhold(&mut self, frame: CapturedFrame) -> CaptureOutcome {
        let (target, captured_at, width, height) = (
            frame.target.clone(),
            frame.captured_at,
            frame.width,
            frame.height,
        );
        drop(frame);
        self.forget_window(&target.window);
        withheld(target, captured_at, width, height)
    }

    pub fn forget_window(&mut self, window: &WindowIdent) {
        self.tile_hashes.pop(window);
        self.line_caches.forget_window(window);
    }
}

/// The outcome of a capture whose window showed a password field: nothing to report.
const fn withheld(
    target: CaptureTarget,
    captured_at: Timestamp,
    width: u32,
    height: u32,
) -> CaptureOutcome {
    CaptureOutcome {
        target,
        captured_at,
        width,
        height,
        damage_permille: 0,
        text: None,
        description: None,
        secret_found: false,
        withheld: true,
        cancelled: false,
    }
}

fn copy_region(frame: &CapturedFrame, bounds: Rect) -> Option<RegionCopy> {
    RegionCopy::of(frame, bounds)
        .map_err(|error| tracing::warn!(%error, "no copy of the region for the models"))
        .ok()
}

/// The secret detectors over all text of the capture at once, so a key that spans two lines is
/// still found.
fn contains_secret(outcome: &CaptureOutcome) -> bool {
    let mut all_text = outcome.target.title.clone();
    all_text.push('\n');
    for line in outcome.text.iter().flat_map(|text| &text.lines) {
        all_text.push_str(line);
        all_text.push('\n');
    }
    if let Some(description) = &outcome.description {
        all_text.push_str(&description.caption);
        all_text.push('\n');
        for entity in &description.visible_entities {
            all_text.push_str(entity);
            all_text.push('\n');
        }
    }
    !context_policy::find_secrets(&all_text).is_empty()
}

#[cfg(test)]
mod tests;
