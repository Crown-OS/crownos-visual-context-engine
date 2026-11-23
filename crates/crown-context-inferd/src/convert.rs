use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use context_infer::{
    EmbedPurpose, EmbeddedTexts, Image, InferError, Job, JobCancel, JobLease, PixelFormat,
    Priority, ScreenDescription, SharedBytes, TextLine,
};
use context_protocol::{
    EmbeddedBatch, InferFailure, JobPriority, JobTicket, RecognizedLine, ScreenSummary,
    SharedImage, TextPurpose, WirePixelFormat,
};
use context_shm::ReadOnlyMapping;

/// Ten minutes: a longer budget is cut to this.
const MAX_BUDGET_MS: u32 = 600_000;
const MAX_SIDE: u32 = 16_384;
/// 64 megapixels: two 8K screens.
const MAX_PIXELS: u64 = 64 << 20;

/// How a running job can end early, and what it holds until its model call returns: its place
/// at the sleep gate and its admission permit. The router keeps the lease with the model call,
/// so a cancelled job still counts until its memfds are unmapped.
#[derive(Debug, Clone)]
pub(crate) struct JobControl {
    pub(crate) cancel: JobCancel,
    pub(crate) lease: JobLease,
}

pub(crate) fn job(ticket: JobTicket, control: JobControl) -> Job {
    let budget = ticket.budget_ms.min(MAX_BUDGET_MS);
    Job {
        deadline: Instant::now() + Duration::from_millis(u64::from(budget)),
        priority: match ticket.priority {
            JobPriority::Background => Priority::Background,
            JobPriority::Interactive => Priority::Interactive,
        },
        cancel: control.cancel,
        lease: control.lease,
    }
}

/// Maps the sealed memfd read-only; the image borrows the mapping, no pixel is copied. The
/// size must be sane, and the memfd no larger than `stride` x `height`.
pub(crate) fn image(layout: SharedImage, pixels: &OwnedFd) -> Result<Image, InferFailure> {
    let pixel_count = u64::from(layout.width) * u64::from(layout.height);
    if layout.width == 0
        || layout.height == 0
        || layout.width.max(layout.height) > MAX_SIDE
        || pixel_count > MAX_PIXELS
    {
        return Err(InferFailure::InvalidInput(format!(
            "a {}x{} image",
            layout.width, layout.height
        )));
    }
    let limit = layout.stride as usize * layout.height as usize;
    let mapping = ReadOnlyMapping::map(pixels, limit)
        .map_err(|error| InferFailure::InvalidInput(error.to_string()))?;
    let format = match layout.format {
        WirePixelFormat::Rgb8 => PixelFormat::Rgb8,
        WirePixelFormat::Xrgb8888 => PixelFormat::Xrgb8888,
        WirePixelFormat::Xbgr8888 => PixelFormat::Xbgr8888,
    };
    Image::new(
        layout.width,
        layout.height,
        layout.stride as usize,
        format,
        SharedBytes::new(mapping),
    )
    .map_err(|error| InferFailure::InvalidInput(error.to_string()))
}

pub(crate) fn failure(error: InferError) -> InferFailure {
    match error {
        InferError::Unavailable(what) => InferFailure::Unavailable(what.into_owned()),
        InferError::Timeout => InferFailure::Timeout,
        InferError::Backend(message) => InferFailure::Backend(message),
        InferError::Schema(message) => InferFailure::Schema(message),
        InferError::Busy { retry_after } => InferFailure::Busy {
            retry_after_ms: u32::try_from(retry_after.as_millis()).unwrap_or(u32::MAX),
        },
        InferError::InvalidInput(message) => InferFailure::InvalidInput(message),
        InferError::Cancelled => InferFailure::Cancelled,
    }
}

pub(crate) const fn purpose(purpose: TextPurpose) -> EmbedPurpose {
    match purpose {
        TextPurpose::Query => EmbedPurpose::Query,
        TextPurpose::Document => EmbedPurpose::Document,
    }
}

pub(crate) fn lines(lines: Vec<TextLine>) -> Vec<RecognizedLine> {
    lines
        .into_iter()
        .map(|line| RecognizedLine {
            text: line.text,
            confidence: line.confidence,
            x: line.bounds.x,
            y: line.bounds.y,
            width: line.bounds.width,
            height: line.bounds.height,
        })
        .collect()
}

pub(crate) fn embedded(embedded: EmbeddedTexts) -> EmbeddedBatch {
    EmbeddedBatch {
        space: embedded.space.to_string(),
        dimensions: embedded.space.dimensions,
        values: embedded.embeddings.values,
    }
}

pub(crate) fn screen(description: ScreenDescription) -> ScreenSummary {
    let content_kind = serde_json::to_value(description.content_kind)
        .ok()
        .and_then(|kind| kind.as_str().map(str::to_owned))
        .unwrap_or_default();
    ScreenSummary {
        content_kind,
        caption: description.caption,
        visible_entities: description.visible_entities,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use context_shm::share_read_only;

    #[test]
    fn a_shared_frame_maps_without_a_copy_and_reads_as_rgb() {
        let memfd = share_read_only(c"frame", &[3, 2, 1, 0, 6, 5, 4, 0]).unwrap();
        let layout = SharedImage {
            width: 2,
            height: 1,
            stride: 8,
            format: WirePixelFormat::Xrgb8888,
        };
        let frame = image(layout, &memfd).unwrap();
        assert_eq!(frame.to_rgb8(), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn zero_and_huge_sizes_and_oversized_memfds_are_refused() {
        let memfd = share_read_only(c"frame", &[0; 64]).unwrap();
        let layout = |width, height, stride| SharedImage {
            width,
            height,
            stride,
            format: WirePixelFormat::Xrgb8888,
        };
        for refused in [layout(0, 4, 16), layout(20_000, 1, 80_000), layout(2, 2, 8)] {
            assert!(matches!(
                image(refused, &memfd),
                Err(InferFailure::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn a_layout_larger_than_the_memfd_is_refused() {
        let memfd = share_read_only(c"frame", &[0; 8]).unwrap();
        let layout = SharedImage {
            width: 4,
            height: 4,
            stride: 16,
            format: WirePixelFormat::Xrgb8888,
        };
        assert!(matches!(
            image(layout, &memfd),
            Err(InferFailure::InvalidInput(_))
        ));
    }
}
