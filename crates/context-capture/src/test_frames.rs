#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use context_core::{Trigger, WindowIdent};

use crate::frame::{CaptureTarget, CapturedFrame};
use crate::geometry::Rect;
use crate::util::frame_memory::FrameMemory;

pub(crate) const BACKGROUND_BGRX: [u8; 4] = [0xF0, 0xF0, 0xF0, 0xFF];
pub(crate) const INK_BGRX: [u8; 4] = [0x30, 0x20, 0x10, 0xFF];

pub(crate) fn target(window: &str) -> CaptureTarget {
    CaptureTarget {
        window: WindowIdent(window.to_owned()),
        trigger: Trigger::FocusChange,
        app_key: "test.app".to_owned(),
        app_name: "Test".to_owned(),
        title: "Test window".to_owned(),
    }
}

pub(crate) fn frame_for(
    target: CaptureTarget,
    width: u32,
    height: u32,
    painter: impl FnOnce(&mut [u8]),
    damage: Vec<Rect>,
) -> CapturedFrame {
    let stride = width as usize * CapturedFrame::BYTES_PER_PIXEL;
    let mut pixels = BACKGROUND_BGRX.repeat(width as usize * height as usize);
    painter(&mut pixels);
    let mut memory = FrameMemory::allocate(pixels.len()).unwrap();
    assert!(memory.write_at(0, &pixels));
    memory.seal().unwrap();
    CapturedFrame::new(target, width, height, stride, damage, memory).unwrap()
}

pub(crate) fn frame_with(
    width: u32,
    height: u32,
    painter: impl FnOnce(&mut [u8]),
    damage: Vec<Rect>,
) -> CapturedFrame {
    frame_for(target("window"), width, height, painter, damage)
}

pub(crate) fn frame_for_window(window: &str, width: u32, height: u32) -> CapturedFrame {
    frame_for(target(window), width, height, |_| {}, vec![])
}

pub(crate) fn paint(pixels: &mut [u8], width: u32, x: u32, y: u32) {
    let start = (y as usize * width as usize + x as usize) * 4;
    pixels[start..start + 4].copy_from_slice(&INK_BGRX);
}

pub(crate) fn paint_rect(pixels: &mut [u8], width: u32, rect: Rect) {
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            paint(pixels, width, x, y);
        }
    }
}

/// A second descriptor of the frame's memfd, to read it after the frame is gone.
pub(crate) fn memfd_reader(frame: &CapturedFrame) -> std::os::fd::OwnedFd {
    rustix::io::dup(frame.memory().memfd()).unwrap()
}

pub(crate) fn is_all_zero(reader: &std::os::fd::OwnedFd) -> bool {
    let length = usize::try_from(rustix::fs::fstat(reader).unwrap().st_size).unwrap();
    let mut contents = vec![0xFF; length];
    let read = rustix::io::pread(reader, &mut contents, 0).unwrap();
    read == length && contents.iter().all(|byte| *byte == 0)
}
