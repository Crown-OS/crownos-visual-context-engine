//! Frames that wait for analysis, in RAM only. The queue is small and short-lived: a frame that
//! expires or does not fit is dropped (and so zeroed), counted, and never taken again.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::frame::CapturedFrame;

pub const MAX_QUEUED_FRAMES: usize = 3;
pub const MAX_QUEUED_BYTES: usize = 64 << 20;
pub const FRAME_TIME_TO_LIVE: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub struct FrameQueue {
    frames: VecDeque<CapturedFrame>,
    queued_bytes: usize,
    dropped_unanalysed: u64,
}

impl FrameQueue {
    /// Queues `frame`. A queued frame of the same window is replaced; then the oldest frames go
    /// until the queue is within its frame and byte limits.
    pub fn push(&mut self, frame: CapturedFrame, now: Instant) {
        self.expire(now);
        if let Some(position) = self
            .frames
            .iter()
            .position(|queued| queued.target.window == frame.target.window)
        {
            self.drop_at(position);
        }
        if frame.byte_len() > MAX_QUEUED_BYTES {
            self.dropped_unanalysed += 1;
            return;
        }
        self.queued_bytes += frame.byte_len();
        self.frames.push_back(frame);
        while self.frames.len() > MAX_QUEUED_FRAMES || self.queued_bytes > MAX_QUEUED_BYTES {
            self.drop_at(0);
        }
    }

    /// The oldest frame that has not expired.
    pub fn pop(&mut self, now: Instant) -> Option<CapturedFrame> {
        self.expire(now);
        let frame = self.frames.pop_front()?;
        self.queued_bytes -= frame.byte_len();
        Some(frame)
    }

    /// Drops every queued frame, as before sleep.
    pub fn clear(&mut self) {
        while !self.frames.is_empty() {
            self.drop_at(0);
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub const fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub const fn dropped_unanalysed(&self) -> u64 {
        self.dropped_unanalysed
    }

    pub(crate) fn expire(&mut self, now: Instant) {
        while self.frames.front().is_some_and(|frame| {
            now.saturating_duration_since(frame.received) >= FRAME_TIME_TO_LIVE
        }) {
            self.drop_at(0);
        }
    }

    fn drop_at(&mut self, position: usize) {
        if let Some(frame) = self.frames.remove(position) {
            self.queued_bytes -= frame.byte_len();
            self.dropped_unanalysed += 1;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_frames::{frame_for_window, frame_with, is_all_zero, memfd_reader, paint};

    #[test]
    fn the_latest_frame_of_a_window_wins() {
        let mut queue = FrameQueue::default();
        let now = Instant::now();
        queue.push(frame_for_window("a", 4, 4), now);
        queue.push(frame_for_window("a", 8, 8), now);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dropped_unanalysed(), 1);
        assert_eq!(queue.pop(now).unwrap().width, 8);
    }

    #[test]
    fn at_most_three_frames_wait() {
        let mut queue = FrameQueue::default();
        let now = Instant::now();
        for window in ["a", "b", "c", "d"] {
            queue.push(frame_for_window(window, 4, 4), now);
        }
        assert_eq!(queue.len(), MAX_QUEUED_FRAMES);
        assert_eq!(queue.dropped_unanalysed(), 1);
        assert_eq!(queue.pop(now).unwrap().target.window.0, "b");
    }

    #[test]
    fn the_byte_limit_drops_the_oldest_frames() {
        let mut queue = FrameQueue::default();
        let now = Instant::now();
        queue.push(frame_for_window("a", 2048, 2048), now);
        queue.push(frame_for_window("b", 2048, 2048), now);
        queue.push(frame_for_window("c", 2048, 2048), now);
        assert!(queue.queued_bytes() <= MAX_QUEUED_BYTES);
        assert_eq!(queue.len(), 3);
        queue.push(frame_for_window("d", 2048, 2048), now);
        assert_eq!(queue.len(), 3);
        queue.push(frame_for_window("e", 4097, 4097), now);
        assert_eq!(queue.dropped_unanalysed(), 2);
        assert!(queue.queued_bytes() <= MAX_QUEUED_BYTES);
    }

    #[test]
    fn expired_frames_are_dropped_and_counted() {
        let mut queue = FrameQueue::default();
        let now = Instant::now();
        queue.push(frame_for_window("a", 4, 4), now);
        assert!(queue.pop(Instant::now() + FRAME_TIME_TO_LIVE).is_none());
        assert_eq!(queue.dropped_unanalysed(), 1);
        assert_eq!(queue.queued_bytes(), 0);
    }

    #[test]
    fn a_dropped_frame_is_zeroed() {
        let mut queue = FrameQueue::default();
        let now = Instant::now();
        let painted = frame_with(8, 8, |pixels| paint(pixels, 8, 1, 1), vec![]);
        let reader = memfd_reader(&painted);
        assert!(!is_all_zero(&reader));
        queue.push(painted, now);
        queue.push(frame_with(8, 8, |_| {}, vec![]), now);
        assert!(is_all_zero(&reader));
    }
}
