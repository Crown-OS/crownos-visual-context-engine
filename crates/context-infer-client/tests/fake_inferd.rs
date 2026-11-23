//! Talks to a fake inferd on a private socket: the frame must arrive sealed and unchanged.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use context_infer::{
    ContentKind, ContextTasks, EmbedPurpose, Image, InferError, Job, PixelFormat, Priority,
    SegmentDigest,
};
use context_infer_client::{InferdClient, SealedFrame};
use context_protocol::inference::{self, Handler};
use context_protocol::{
    EmbeddedBatch, INFERENCE_SERVICE, InferCapabilities, InferFailure, InferReply, JobTicket,
    RecognizedLine, ScreenSummary, SharedImage, TextPurpose,
};
use context_shm::{ReadOnlyMapping, share_read_only};
use crownos_ipc::{RemoteError, ServiceBuilder};

struct FakeInferd;

type Answer<T> = Result<InferReply<T>, RemoteError>;

impl Handler for FakeInferd {
    fn capabilities(&mut self) -> Result<InferCapabilities, RemoteError> {
        Ok(InferCapabilities {
            embed_space: Some("vectors/OnnxRuntime/int8/2".into()),
            ..InferCapabilities::default()
        })
    }

    fn ocr(
        &mut self,
        _: JobTicket,
        image: SharedImage,
        pixels: OwnedFd,
    ) -> Answer<Vec<RecognizedLine>> {
        let mapping = ReadOnlyMapping::map(&pixels, 1 << 20).unwrap();
        Ok(Ok(vec![RecognizedLine {
            text: format!(
                "{}x{} {:?} {:?}",
                image.width,
                image.height,
                image.format,
                mapping.as_ref().get(..4).unwrap()
            ),
            confidence: 0.9,
            x: 0,
            y: 0,
            width: image.width,
            height: image.height,
        }]))
    }

    fn embed_text(
        &mut self,
        _: JobTicket,
        texts: Vec<String>,
        _: TextPurpose,
    ) -> Answer<EmbeddedBatch> {
        if texts.is_empty() {
            return Ok(Err(InferFailure::Busy {
                retry_after_ms: 300,
            }));
        }
        Ok(Ok(EmbeddedBatch {
            space: "vectors/OnnxRuntime/int8/2".into(),
            dimensions: 2,
            values: texts.iter().flat_map(|_| [0.6, 0.8]).collect(),
        }))
    }

    fn rerank(&mut self, _: JobTicket, _: String, _: Vec<String>) -> Answer<Vec<f32>> {
        Ok(Err(InferFailure::Unavailable("rerank".into())))
    }

    fn generate(
        &mut self,
        job: JobTicket,
        template: String,
        vars: Vec<(String, String)>,
        schema: Option<String>,
        _: u32,
    ) -> Answer<String> {
        assert_eq!(template, "summarize_segments");
        assert!(vars[0].1.contains("App: Zed"));
        assert!(schema.unwrap().contains("open_items"));
        assert!(job.budget_ms > 0);
        Ok(Ok(r#"{"segments": [{"summary": "s", "activity_label": "coding", "entities": [], "facts": [], "open_items": []}]}"#.into()))
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
        Ok(Err(InferFailure::Unavailable("images".into())))
    }

    fn describe_screen(
        &mut self,
        _: JobTicket,
        _: SharedImage,
        app_name: String,
        _: String,
        pixels: OwnedFd,
    ) -> Answer<ScreenSummary> {
        ReadOnlyMapping::map(&pixels, 1 << 20).unwrap();
        Ok(Ok(ScreenSummary {
            content_kind: "code".into(),
            caption: format!("Edits code in {app_name}."),
            visible_entities: Vec::new(),
        }))
    }

    fn unload(&mut self) -> Result<u32, RemoteError> {
        Ok(0)
    }
}

fn start_fake(directory: &Path) {
    let mut server = ServiceBuilder::new(INFERENCE_SERVICE)
        .directory(directory)
        .build()
        .unwrap();
    thread::spawn(move || {
        let mut handler = FakeInferd;
        let _ = crownos_ipc::blocking::serve(&mut server, |server, peer, message| {
            inference::dispatch(&mut handler, server, peer, message).map(|()| true)
        });
    });
}

fn job() -> Job {
    Job::within(Duration::from_secs(5), Priority::Interactive)
}

#[test]
fn every_task_reaches_inferd_with_sealed_frames() {
    let directory: PathBuf =
        std::env::temp_dir().join(format!("inferd-client-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    start_fake(&directory);
    let client = InferdClient::new(Some(directory.clone()));

    let pixels = vec![3, 2, 1, 0, 6, 5, 4, 0];
    let frame = SealedFrame::from_sealed_memfd(
        share_read_only(c"frame", &pixels).unwrap(),
        2,
        1,
        8,
        PixelFormat::Xrgb8888,
    )
    .unwrap();
    let lines = client.recognize_frame(&frame, job()).unwrap();
    assert_eq!(lines[0].text, "2x1 Xrgb8888 [3, 2, 1, 0]");

    let image = Image::new(2, 1, 8, PixelFormat::Xrgb8888, pixels.into()).unwrap();
    let description = client
        .describe_screen(&image, "Zed", "main.rs", job())
        .unwrap();
    assert_eq!(description.content_kind, ContentKind::Code);

    let segment = SegmentDigest {
        app: "Zed".into(),
        ..SegmentDigest::default()
    };
    assert_eq!(
        client.summarize_segments(&[segment], job()).unwrap().len(),
        1
    );

    let embedded = client
        .embed_text(vec!["a".into()], EmbedPurpose::Document, job())
        .unwrap();
    assert_eq!(embedded.space.to_string(), "vectors/OnnxRuntime/int8/2");
    assert_eq!(client.embed_space(), Some(embedded.space));
    assert!(matches!(
        client.embed_text(Vec::new(), EmbedPurpose::Document, job()),
        Err(InferError::Busy { retry_after }) if retry_after == Duration::from_millis(300)
    ));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_unsealed_memfd_is_refused_before_it_is_sent() {
    let open = std::fs::File::open("/dev/null").unwrap();
    assert!(matches!(
        SealedFrame::from_sealed_memfd(open.into(), 1, 1, 4, PixelFormat::Xrgb8888),
        Err(InferError::InvalidInput(_))
    ));
}

#[test]
fn no_daemon_is_unavailable() {
    let client = InferdClient::new(Some(PathBuf::from("/nonexistent/inferd")));
    let segment = SegmentDigest::default();
    assert!(matches!(
        client.summarize_segments(&[segment], job()),
        Err(InferError::Unavailable(_))
    ));
}
