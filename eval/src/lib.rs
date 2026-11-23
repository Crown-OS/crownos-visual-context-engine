//! A working day that never happened: windows, focus changes, typing, copies, an idle lunch and
//! a lock, generated from a seed so every run of the engine's tests sees the same stream and the
//! gold segments it should recover. Nothing here comes from a real screen.

mod gold;
pub mod pixel_scan;
mod scenario;
mod trace;
mod util;

pub use gold::GoldSegment;
pub use scenario::{Scenario, generate};
pub use trace::Trace;
