//! [`ScreenModels`] over crown-context-inferd. A region copy goes over as a sealed memfd in
//! `XRGB8888`, so no pixel is converted or copied again on the way.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use context_infer::{InferError, Job, JobCancel, PixelFormat, Priority};
use context_infer_client::{InferdClient, SealedFrame};

use crate::cancellation::ModelCall;
use crate::geometry::Rect;
use crate::region_copy::RegionCopy;
use crate::screen_models::{ModelError, RecognizedLine, ScreenModels, ScreenSummary};

#[derive(Debug)]
pub struct InferScreenModels {
    client: InferdClient,
    unreachable_reported: AtomicBool,
}

impl InferScreenModels {
    pub const fn new(client: InferdClient) -> Self {
        Self {
            client,
            unreachable_reported: AtomicBool::new(false),
        }
    }

    /// Runs `attempt` once, and once more when inferd is busy and the deadline leaves time for
    /// the wait it asks for. Each attempt runs on its own thread, so a cancel returns at once,
    /// also during the wait. Without inferd, one warning is logged and every call fails as
    /// unavailable, which leaves captures to metadata.
    fn call<T: Send + 'static>(
        &self,
        call: &ModelCall<'_>,
        attempt: impl Fn(InferdClient, Job) -> Result<T, InferError> + Clone + Send + 'static,
    ) -> Result<T, ModelError> {
        let run = || {
            let client = self.client.clone();
            let attempt = attempt.clone();
            let cancel = JobCancel::default();
            let job = Job {
                deadline: call.deadline,
                priority: Priority::Interactive,
                cancel: cancel.clone(),
            };
            let answer = call.wait_for(move || attempt(client, job));
            if answer.is_none() {
                cancel.cancel();
            }
            answer.ok_or(ModelError::Cancelled)
        };
        match run()? {
            Err(InferError::Busy { retry_after })
                if Instant::now() + retry_after < call.deadline =>
            {
                if !call.sleep(retry_after) {
                    return Err(ModelError::Cancelled);
                }
                run()?.map_err(|error| self.model_error(error))
            }
            other => other.map_err(|error| self.model_error(error)),
        }
    }

    fn model_error(&self, error: InferError) -> ModelError {
        match error {
            InferError::Unavailable(what) => {
                if !self.unreachable_reported.swap(true, Ordering::Relaxed) {
                    tracing::warn!(%what, "no model for captures; only frame metadata is kept");
                }
                ModelError::Unavailable(what.into_owned())
            }
            other => ModelError::Failed(other.to_string()),
        }
    }
}

impl ScreenModels for InferScreenModels {
    fn recognize_text(
        &self,
        region: &RegionCopy,
        call: &ModelCall<'_>,
    ) -> Result<Vec<RecognizedLine>, ModelError> {
        let frame = Arc::new(sealed_frame(region)?);
        let origin = region.bounds();
        let lines = self.call(call, move |client, job| {
            client.recognize_frame(&frame, &job)
        })?;
        Ok(lines
            .into_iter()
            .map(|line| RecognizedLine {
                text: line.text,
                confidence: line.confidence,
                bounds: Rect::new(
                    origin.x.saturating_add(line.bounds.x),
                    origin.y.saturating_add(line.bounds.y),
                    line.bounds.width,
                    line.bounds.height,
                ),
            })
            .collect())
    }

    fn describe_screen(
        &self,
        region: &RegionCopy,
        app_name: &str,
        window_title: &str,
        call: &ModelCall<'_>,
    ) -> Result<ScreenSummary, ModelError> {
        let frame = Arc::new(sealed_frame(region)?);
        let (app_name, window_title) = (app_name.to_owned(), window_title.to_owned());
        let description = self.call(call, move |client, job| {
            client.describe_frame(&frame, &app_name, &window_title, &job)
        })?;
        Ok(ScreenSummary {
            content_kind: format!("{:?}", description.content_kind).to_ascii_lowercase(),
            caption: description.caption,
            visible_entities: description.visible_entities,
        })
    }
}

fn sealed_frame(region: &RegionCopy) -> Result<SealedFrame, ModelError> {
    let bounds = region.bounds();
    let memfd = region
        .memfd()
        .try_clone_to_owned()
        .map_err(|error| ModelError::Failed(format!("duplicating a region copy: {error}")))?;
    SealedFrame::from_sealed_memfd(
        memfd,
        bounds.width,
        bounds.height,
        region.stride(),
        PixelFormat::Xrgb8888,
    )
    .map_err(|error| ModelError::Failed(error.to_string()))
}

#[cfg(test)]
mod tests;
