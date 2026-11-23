//! What the analysis asks of the model layer. The capture code depends on this trait only; one
//! adapter maps it onto crown-context-inferd.

use std::time::Instant;

use crate::geometry::Rect;
use crate::region_copy::RegionCopy;

#[derive(Clone, Debug, PartialEq)]
pub struct RecognizedLine {
    pub text: String,
    pub confidence: f32,
    /// In frame pixels.
    pub bounds: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSummary {
    pub content_kind: String,
    pub caption: String,
    pub visible_entities: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("no model can do this now: {0}")]
    Unavailable(String),
    #[error("the model failed: {0}")]
    Failed(String),
}

pub trait ScreenModels: Send + Sync {
    /// The text lines in the region, in reading order.
    ///
    /// # Errors
    ///
    /// Fails when no OCR model is usable or the deadline passes.
    fn recognize_text(
        &self,
        region: &RegionCopy,
        deadline: Instant,
    ) -> Result<Vec<RecognizedLine>, ModelError>;

    /// # Errors
    ///
    /// Fails when no vision model is usable or the deadline passes.
    fn describe_screen(
        &self,
        region: &RegionCopy,
        app_name: &str,
        window_title: &str,
        deadline: Instant,
    ) -> Result<ScreenSummary, ModelError>;
}

/// For a machine with no model: captures still update the tiles and the frame metadata.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoScreenModels;

impl ScreenModels for NoScreenModels {
    fn recognize_text(
        &self,
        _: &RegionCopy,
        _: Instant,
    ) -> Result<Vec<RecognizedLine>, ModelError> {
        Err(ModelError::Unavailable("OCR".to_owned()))
    }

    fn describe_screen(
        &self,
        _: &RegionCopy,
        _: &str,
        _: &str,
        _: Instant,
    ) -> Result<ScreenSummary, ModelError> {
        Err(ModelError::Unavailable("a vision model".to_owned()))
    }
}
