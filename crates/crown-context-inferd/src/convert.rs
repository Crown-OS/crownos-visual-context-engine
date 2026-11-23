use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use context_infer::{
    EmbedPurpose, EmbeddedTexts, Image, InferError, Job, PixelFormat, Priority, ScreenDescription,
    SharedBytes, TextLine,
};
use context_protocol::{
    EmbeddedBatch, InferFailure, JobPriority, JobTicket, RecognizedLine, ScreenSummary,
    SharedImage, TextPurpose, WirePixelFormat,
};
use context_shm::ReadOnlyMapping;

pub(crate) fn job(ticket: JobTicket) -> Job {
    Job {
        deadline: Instant::now() + Duration::from_millis(u64::from(ticket.budget_ms)),
        priority: match ticket.priority {
            JobPriority::Background => Priority::Background,
            JobPriority::Interactive => Priority::Interactive,
        },
    }
}

/// Maps the sealed memfd read-only; the image borrows the mapping, no pixel is copied.
pub(crate) fn image(layout: SharedImage, pixels: &OwnedFd) -> Result<Image, InferFailure> {
    let mapping = ReadOnlyMapping::map(pixels)
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
