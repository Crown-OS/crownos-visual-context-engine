#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use context_protocol::inference::{self, Handler};
use context_protocol::{
    EmbeddedBatch, INFERENCE_SERVICE, InferCapabilities, InferFailure, InferReply, JobTicket,
    RecognizedLine as WireLine, ScreenSummary as WireSummary, SharedImage, TextPurpose,
};
use context_shm::ReadOnlyMapping;
use crownos_ipc::{RemoteError, ServiceBuilder};

use super::*;
use crate::analysis::{AnalysisConditions, FrameAnalyzer};
use crate::structured_text::NoStructuredText;
use crate::test_frames::{frame_for, is_all_zero, memfd_reader, paint_rect, target};

const PLANTED: &str = "fn main() {";

type Answer<T> = Result<InferReply<T>, RemoteError>;

/// Answers `Busy` to the first `busy_answers` OCR calls, then the planted line. It notes
/// whether the capture memfd was already zero when OCR ran.
#[derive(Default)]
struct FakeInferd {
    busy_answers: usize,
    retry_after_ms: u32,
    ocr_calls: Arc<AtomicUsize>,
    capture_memfd: Arc<Mutex<Option<OwnedFd>>>,
    capture_was_zero: Arc<AtomicBool>,
}

impl Handler for FakeInferd {
    fn capabilities(&mut self) -> Result<InferCapabilities, RemoteError> {
        Ok(InferCapabilities::default())
    }

    fn ocr(&mut self, _: JobTicket, image: SharedImage, pixels: OwnedFd) -> Answer<Vec<WireLine>> {
        let call = self.ocr_calls.fetch_add(1, Ordering::SeqCst);
        if call < self.busy_answers {
            return Ok(Err(InferFailure::Busy {
                retry_after_ms: self.retry_after_ms,
            }));
        }
        let length = image.stride as usize * image.height as usize;
        assert!(ReadOnlyMapping::map(&pixels, length).is_ok());
        if let Some(capture) = self.capture_memfd.lock().unwrap().as_ref() {
            self.capture_was_zero
                .store(is_all_zero(capture), Ordering::SeqCst);
        }
        Ok(Ok(vec![WireLine {
            text: PLANTED.to_owned(),
            confidence: 0.95,
            x: 0,
            y: 0,
            width: image.width,
            height: image.height,
        }]))
    }

    fn embed_text(
        &mut self,
        _: JobTicket,
        _: Vec<String>,
        _: TextPurpose,
    ) -> Answer<EmbeddedBatch> {
        Ok(Err(InferFailure::Unavailable("embeddings".into())))
    }

    fn rerank(&mut self, _: JobTicket, _: String, _: Vec<String>) -> Answer<Vec<f32>> {
        Ok(Err(InferFailure::Unavailable("rerank".into())))
    }

    fn generate(
        &mut self,
        _: JobTicket,
        _: String,
        _: Vec<(String, String)>,
        _: Option<String>,
        _: u32,
    ) -> Answer<String> {
        Ok(Err(InferFailure::Unavailable("generation".into())))
    }

    fn generate_with_image(
        &mut self,
        _: JobTicket,
        _: String,
        _: Vec<(String, String)>,
        _: Option<String>,
        _: u32,
        _: SharedImage,
        _: OwnedFd,
    ) -> Answer<String> {
        Ok(Err(InferFailure::Unavailable("generation".into())))
    }

    fn describe_screen(
        &mut self,
        _: JobTicket,
        _: SharedImage,
        _: String,
        _: String,
        _: OwnedFd,
    ) -> Answer<WireSummary> {
        Ok(Err(InferFailure::Unavailable("vision".into())))
    }

    fn unload(&mut self) -> Result<u32, RemoteError> {
        Ok(0)
    }

    fn cancel(&mut self, _: u64) -> Result<bool, RemoteError> {
        Ok(false)
    }
}

fn serve(name: &str, mut fake: FakeInferd) -> (InferScreenModels, FakeInferd, PathBuf) {
    let directory: PathBuf =
        std::env::temp_dir().join(format!("vcge-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut server = ServiceBuilder::new(INFERENCE_SERVICE)
        .directory(&directory)
        .build()
        .unwrap();
    let shared = FakeInferd {
        busy_answers: fake.busy_answers,
        retry_after_ms: fake.retry_after_ms,
        ocr_calls: Arc::clone(&fake.ocr_calls),
        capture_memfd: Arc::clone(&fake.capture_memfd),
        capture_was_zero: Arc::clone(&fake.capture_was_zero),
    };
    std::thread::spawn(move || {
        let _ = crownos_ipc::blocking::serve(&mut server, |server, peer, message| {
            inference::dispatch(&mut fake, server, peer, message).map(|()| true)
        });
    });
    (
        InferScreenModels::new(InferdClient::new(Some(directory.clone()))),
        shared,
        directory,
    )
}

fn painted_frame() -> crate::frame::CapturedFrame {
    frame_for(
        target("editor"),
        120,
        40,
        |pixels| paint_rect(pixels, 120, Rect::new(10, 10, 60, 10)),
        vec![],
    )
}

#[test]
fn a_busy_inferd_is_asked_again_and_the_frame_is_zero_before_ocr_runs() {
    let (models, fake, directory) = serve(
        "busy",
        FakeInferd {
            busy_answers: 1,
            retry_after_ms: 100,
            ..FakeInferd::default()
        },
    );
    let frame = painted_frame();
    *fake.capture_memfd.lock().unwrap() = Some(memfd_reader(&frame));
    let mut analyzer = FrameAnalyzer::new(Arc::new(models), Box::new(NoStructuredText));
    let cancel = AtomicBool::new(false);
    let outcome = analyzer.analyse(
        frame,
        AnalysisConditions {
            on_battery: true,
            cancel: &cancel,
        },
    );
    assert_eq!(outcome.text.unwrap().lines, [PLANTED]);
    assert_eq!(fake.ocr_calls.load(Ordering::SeqCst), 2);
    assert!(fake.capture_was_zero.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_cancel_ends_the_busy_wait_at_once() {
    let (models, fake, directory) = serve(
        "cancel",
        FakeInferd {
            busy_answers: 1,
            retry_after_ms: 3_000,
            ..FakeInferd::default()
        },
    );
    let frame = painted_frame();
    let copy = RegionCopy::of(&frame, Rect::new(0, 0, 120, 40)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    std::thread::spawn({
        let cancel = Arc::clone(&cancel);
        move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::SeqCst);
        }
    });
    let started = Instant::now();
    let call = ModelCall {
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: cancel.as_ref(),
    };
    let result = models.recognize_text(&copy, &call);
    assert!(matches!(result, Err(ModelError::Cancelled)));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(fake.ocr_calls.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(directory);
}
