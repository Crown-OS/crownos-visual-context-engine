use zeroize::Zeroize;

use crate::frame::CapturedFrame;

/// Pixels copied out of a frame one row at a time. The buffer is zeroed before it grows and
/// when it drops, so no copy of a row outlives the analysis.
#[derive(Debug, Default)]
pub(crate) struct RowBuffer {
    bytes: Vec<u8>,
}

impl RowBuffer {
    /// `width` pixels of row `y` from column `x`, or `None` outside the frame.
    pub(crate) fn read(
        &mut self,
        frame: &CapturedFrame,
        x: u32,
        y: u32,
        width: u32,
    ) -> Option<&[u8]> {
        let length = width as usize * CapturedFrame::BYTES_PER_PIXEL;
        if self.bytes.len() < length {
            self.bytes.zeroize();
            self.bytes = vec![0; length];
        }
        let row = self.bytes.get_mut(..length)?;
        frame.copy_row(x, y, row).then_some(&*row)
    }
}

impl Drop for RowBuffer {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}
