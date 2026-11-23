//! Analyse, then delete. The focused window is captured into a sealed, locked memfd; the
//! compositor's damage picks the 32 px tiles to hash; changed tiles, minus the regions an app
//! changes all the time, make one crop; text comes from the window or from OCR of that crop,
//! line by line through a text-only cache; frames with little text get a caption; a secret drops
//! all text and flags the window. Then the frame is zeroed and closed. Only text and numbers
//! leave this crate.

mod analysis;
mod capture_hold;
mod crop;
mod frame;
mod frame_inbox;
mod frame_queue;
mod geometry;
mod image_regions;
mod infer_models;
mod line_bands;
mod line_cache;
mod metrics;
mod region_copy;
mod schedule;
mod screen_models;
mod service;
mod session;
mod structured_text;
mod tiles;
mod util;
mod volatile_mask;
mod worker;

#[cfg(test)]
mod test_frames;

pub use analysis::{AnalysisConditions, CAPTURE_ANALYSIS_BUDGET, CaptureOutcome, FrameAnalyzer};
pub use capture_hold::CaptureHold;
pub use frame::{CaptureTarget, CapturedFrame, FrameLayoutError};
pub use frame_queue::{FRAME_TIME_TO_LIVE, FrameQueue, MAX_QUEUED_BYTES, MAX_QUEUED_FRAMES};
pub use geometry::Rect;
pub use infer_models::InferScreenModels;
pub use region_copy::{RegionCopy, RegionCopyError};
pub use schedule::CaptureScheduler;
pub use screen_models::{ModelError, NoScreenModels, RecognizedLine, ScreenModels, ScreenSummary};
pub use service::{CaptureCounts, CaptureService, CaptureServiceParts};
pub use session::SessionError;
pub use structured_text::{NoStructuredText, StructuredLines, StructuredText};
pub use util::frame_memory::{FrameMemory, FrameMemoryError};
pub use worker::{OutcomeSink, PowerProbe};
