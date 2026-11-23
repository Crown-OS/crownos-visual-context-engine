//! Wire types of the inference service. Images travel in a sealed memfd next to the message,
//! never in the payload.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum JobPriority {
    #[default]
    Background,
    Interactive,
}

/// Every job names itself, its urgency and how long its answer is useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobTicket {
    pub job_id: u64,
    pub priority: JobPriority,
    /// Milliseconds from the send; the job gives `Timeout` after that.
    pub budget_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WirePixelFormat {
    Rgb8,
    Xrgb8888,
    Xbgr8888,
}

/// The layout of the pixels in the memfd that comes with the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedImage {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: WirePixelFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InferFailure {
    Unavailable(String),
    Timeout,
    Backend(String),
    Schema(String),
    /// Too many jobs run now; send again after this time.
    Busy {
        retry_after_ms: u32,
    },
    InvalidInput(String),
}

pub type InferReply<T> = Result<T, InferFailure>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecognizedLine {
    pub text: String,
    pub confidence: f32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextPurpose {
    Query,
    Document,
}

/// Row-major vectors. A change of `space` means stored vectors must be made again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedBatch {
    pub space: String,
    pub dimensions: u32,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSummary {
    pub content_kind: String,
    pub caption: String,
    pub visible_entities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRoute {
    pub task: String,
    /// `model@Runtime/Device`, best first, after the probe and the policy.
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InferCapabilities {
    /// `Runtime/Device` pairs whose probe passed.
    pub devices: Vec<String>,
    pub routes: Vec<TaskRoute>,
    /// `capability model@Runtime/Device` for each model in memory.
    pub loaded: Vec<String>,
    pub embed_space: Option<String>,
    pub templates: Vec<String>,
}
