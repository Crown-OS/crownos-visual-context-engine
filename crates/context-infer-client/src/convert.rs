use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use context_infer::{
    ContentKind, EmbedPurpose, EmbedSpace, EmbeddedTexts, Embeddings, InferError, Job, Priority,
    ScreenDescription, TextBounds, TextLine,
};
use context_protocol::{
    EmbeddedBatch, InferFailure, JobPriority, JobTicket, RecognizedLine, ScreenSummary, TextPurpose,
};

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// The job as the protocol carries it: the deadline becomes the time left, at least 1 ms.
pub(crate) fn ticket(job: &Job) -> JobTicket {
    let budget = job.deadline.saturating_duration_since(Instant::now());
    JobTicket {
        job_id: NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed),
        priority: match job.priority {
            Priority::Background => JobPriority::Background,
            Priority::Interactive => JobPriority::Interactive,
        },
        budget_ms: u32::try_from(budget.as_millis()).unwrap_or(u32::MAX).max(1),
    }
}

pub(crate) fn error(failure: InferFailure) -> InferError {
    match failure {
        InferFailure::Unavailable(what) => InferError::Unavailable(what.into()),
        InferFailure::Timeout => InferError::Timeout,
        InferFailure::Backend(message) => InferError::Backend(message),
        InferFailure::Schema(message) => InferError::Schema(message),
        InferFailure::Busy { retry_after_ms } => InferError::Busy {
            retry_after: Duration::from_millis(u64::from(retry_after_ms)),
        },
        InferFailure::InvalidInput(message) => InferError::InvalidInput(message),
        InferFailure::Cancelled => InferError::Cancelled,
    }
}

pub(crate) const fn purpose(purpose: EmbedPurpose) -> TextPurpose {
    match purpose {
        EmbedPurpose::Query => TextPurpose::Query,
        EmbedPurpose::Document => TextPurpose::Document,
    }
}

pub(crate) fn lines(lines: Vec<RecognizedLine>) -> Vec<TextLine> {
    lines
        .into_iter()
        .map(|line| TextLine {
            text: line.text,
            confidence: line.confidence,
            bounds: TextBounds {
                x: line.x,
                y: line.y,
                width: line.width,
                height: line.height,
            },
        })
        .collect()
}

pub(crate) fn embedded(batch: EmbeddedBatch) -> Result<EmbeddedTexts, InferError> {
    Ok(EmbeddedTexts {
        space: batch.space.parse::<EmbedSpace>()?,
        embeddings: Embeddings {
            dimensions: batch.dimensions as usize,
            values: batch.values,
        },
    })
}

pub(crate) fn screen(summary: ScreenSummary) -> Result<ScreenDescription, InferError> {
    let content_kind: ContentKind =
        serde_json::from_value(serde_json::Value::String(summary.content_kind))
            .map_err(|error| InferError::Schema(error.to_string()))?;
    Ok(ScreenDescription {
        content_kind,
        caption: summary.caption,
        visible_entities: summary.visible_entities,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_becomes_the_time_left_and_busy_keeps_its_wait() {
        let job = Job::within(Duration::from_secs(2), Priority::Interactive);
        let ticket = ticket(&job);
        assert!((1_900..=2_000).contains(&ticket.budget_ms));
        assert_eq!(ticket.priority, JobPriority::Interactive);
        let late = Job::within(Duration::ZERO, Priority::Background);
        assert_eq!(super::ticket(&late).budget_ms, 1);
        assert!(matches!(
            error(InferFailure::Busy { retry_after_ms: 250 }),
            InferError::Busy { retry_after } if retry_after == Duration::from_millis(250)
        ));
    }
}
