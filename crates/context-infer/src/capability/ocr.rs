use crate::error::InferError;
use crate::media::RgbImage;

/// An axis-aligned box in image pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub confidence: f32,
    pub bounds: TextBounds,
}

pub trait Ocr: Send + Sync {
    /// The text lines in reading order.
    fn recognize(&self, image: &RgbImage) -> Result<Vec<TextLine>, InferError>;
}
