use std::time::Instant;

use context_core::{Timestamp, Trigger, WindowIdent};

use crate::geometry::Rect;
use crate::util::frame_memory::FrameMemory;

/// The window a capture is for, with what the analysis needs to know about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTarget {
    pub window: WindowIdent,
    pub trigger: Trigger,
    /// When the engine asked for the capture. Structured text must be read after this.
    pub requested_at: Timestamp,
    /// Volatile-region masks are learned per app.
    pub app_key: String,
    pub app_name: String,
    pub title: String,
}

/// One captured frame: `wl_shm` `XRGB8888`, which is B, G, R, X in memory. The pixels exist only
/// in the frame's memfd and are zeroed when the frame drops.
#[derive(Debug)]
pub struct CapturedFrame {
    pub target: CaptureTarget,
    pub captured_at: Timestamp,
    pub received: Instant,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// What the compositor says changed since the previous frame of the same session.
    pub damage: Vec<Rect>,
    memory: FrameMemory,
}

#[derive(Debug, thiserror::Error)]
#[error("a {width}x{height} frame with a stride of {stride} does not fit in {length} bytes")]
pub struct FrameLayoutError {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub length: usize,
}

impl CapturedFrame {
    pub const BYTES_PER_PIXEL: usize = 4;

    /// # Errors
    ///
    /// Fails when the rows do not fit in the memory.
    pub fn new(
        target: CaptureTarget,
        width: u32,
        height: u32,
        stride: usize,
        damage: Vec<Rect>,
        memory: FrameMemory,
    ) -> Result<Self, FrameLayoutError> {
        let row_bytes = width as usize * Self::BYTES_PER_PIXEL;
        let needed = stride.checked_mul(height as usize);
        if stride < row_bytes || needed.is_none_or(|needed| needed > memory.len()) {
            return Err(FrameLayoutError {
                width,
                height,
                stride,
                length: memory.len(),
            });
        }
        Ok(Self {
            target,
            captured_at: Timestamp::now(),
            received: Instant::now(),
            width,
            height,
            stride,
            damage,
            memory,
        })
    }

    pub const fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub const fn byte_len(&self) -> usize {
        self.memory.len()
    }

    pub const fn memory(&self) -> &FrameMemory {
        &self.memory
    }

    /// Copies the pixels of row `y` from column `x` into `out`, as many as fit. Returns `false`
    /// outside the frame.
    pub fn copy_row(&self, x: u32, y: u32, out: &mut [u8]) -> bool {
        let width = out.len() / Self::BYTES_PER_PIXEL;
        if y >= self.height || (x as usize).saturating_add(width) > self.width as usize {
            return false;
        }
        let start = y as usize * self.stride + x as usize * Self::BYTES_PER_PIXEL;
        self.memory.read_at(start, out)
    }

    /// The share of the frame inside the damage, in thousandths. Overlapping rectangles count
    /// twice, and the share stops at 1000.
    pub fn damage_permille(&self) -> u16 {
        let frame_area = self.bounds().area().max(1);
        let damaged: u64 = self.damage.iter().map(|rect| rect.area()).sum();
        u16::try_from(damaged.min(frame_area) * 1000 / frame_area).unwrap_or(1000)
    }
}
