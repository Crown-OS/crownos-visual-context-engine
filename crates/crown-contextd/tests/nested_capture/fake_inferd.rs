//! A stand-in for crown-context-inferd on the test's socket. OCR answers with planted text: a
//! harmless line for the first calls, then a line with an AWS key, so the test sees OCR text
//! reach the store and a secret stop it. A test that plants its secret elsewhere gets the
//! harmless line only.

use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use context_protocol::inference::{self, Handler};
use context_protocol::{
    EmbeddedBatch, INFERENCE_SERVICE, InferCapabilities, InferFailure, InferReply, JobTicket,
    RecognizedLine, ScreenSummary, SharedImage, TextPurpose,
};
use context_shm::ReadOnlyMapping;
use crownos_ipc::{RemoteError, ServiceBuilder};

pub(crate) const HARMLESS_LINE: &str = "release notes for the nested capture test";
pub(crate) const SECRET_LINE: &str = "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";

type Answer<T> = Result<InferReply<T>, RemoteError>;

struct FakeInferd {
    ocr_calls: Arc<AtomicUsize>,
    harmless_calls: usize,
}

impl Handler for FakeInferd {
    fn capabilities(&mut self) -> Result<InferCapabilities, RemoteError> {
        Ok(InferCapabilities::default())
    }

    fn ocr(
        &mut self,
        _: JobTicket,
        image: SharedImage,
        pixels: OwnedFd,
    ) -> Answer<Vec<RecognizedLine>> {
        let sealed = ReadOnlyMapping::map(&pixels, image.stride as usize * image.height as usize);
        if sealed.is_err() {
            return Ok(Err(InferFailure::InvalidInput(
                "the frame is not sealed".into(),
            )));
        }
        let call = self.ocr_calls.fetch_add(1, Ordering::SeqCst);
        let text = if call < self.harmless_calls {
            HARMLESS_LINE
        } else {
            SECRET_LINE
        };
        Ok(Ok(vec![RecognizedLine {
            text: text.to_owned(),
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
    ) -> Answer<ScreenSummary> {
        Ok(Ok(ScreenSummary {
            content_kind: "image".into(),
            caption: "Noise on a test window.".into(),
            visible_entities: Vec::new(),
        }))
    }

    fn unload(&mut self) -> Result<u32, RemoteError> {
        Ok(0)
    }

    fn cancel(&mut self, _: u64) -> Result<bool, RemoteError> {
        Ok(false)
    }
}

/// Serves on its own thread in `directory` for the rest of the process. OCR reads the secret
/// from call `harmless_calls` on. Returns the OCR call count.
pub(crate) fn start(directory: &Path, harmless_calls: usize) -> Arc<AtomicUsize> {
    let ocr_calls = Arc::new(AtomicUsize::new(0));
    let mut server = ServiceBuilder::new(INFERENCE_SERVICE)
        .directory(directory)
        .build()
        .unwrap_or_else(|error| panic!("cannot serve the fake inferd: {error}"));
    std::thread::spawn({
        let ocr_calls = Arc::clone(&ocr_calls);
        move || {
            let mut handler = FakeInferd {
                ocr_calls,
                harmless_calls,
            };
            let _ = crownos_ipc::blocking::serve(&mut server, |server, peer, message| {
                inference::dispatch(&mut handler, server, peer, message).map(|()| true)
            });
        }
    });
    ocr_calls
}
