#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crownos_config::system::ai::{Device, Runtime};

use super::{ContentKind, ContextTasks, RoutedTasks, SegmentDigest};
use crate::capability::{Generate, GenerateCapabilities};
use crate::error::InferError;
use crate::media::RgbImage;
use crate::registry::Registry;
use crate::request::{GenRequest, Generation, Job, Output, Priority};
use crate::router::{LoadSpec, Router, RouterSettings, RuntimeBackend};

const REGISTRY: &str = r#"(
    version: 2,
    models: {
        "vlm": (capabilities: [Generate, Vision, JsonOutput], licence: "Apache-2.0",
                artifacts: [Gguf(model: (path: "vlm.gguf"), quant: "Q4_K_M")]),
    },
    tasks: {
        DescribeScreen: [(model: "vlm", runtime: LlamaCpp, device: Cpu)],
        SummarizeSegments: [(model: "vlm", runtime: LlamaCpp, device: Cpu)],
    },
)"#;

/// Answers every request with `reply` and keeps the last request.
#[derive(Debug, Default)]
struct CannedGenerator {
    reply: Mutex<String>,
    last_request: Mutex<Option<GenRequest>>,
}

impl Generate for CannedGenerator {
    fn capabilities(&self) -> GenerateCapabilities {
        GenerateCapabilities {
            images: true,
            json_schema: true,
            context_tokens: 4096,
        }
    }

    fn stream(
        &self,
        request: &GenRequest,
        _on_text: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<Generation, InferError> {
        *self.last_request.lock().unwrap() = Some(request.clone());
        Ok(Generation {
            text: self.reply.lock().unwrap().clone(),
            ..Generation::default()
        })
    }
}

#[derive(Debug)]
struct CannedRuntime(Arc<CannedGenerator>);

impl RuntimeBackend for CannedRuntime {
    fn runtime(&self) -> Runtime {
        Runtime::LlamaCpp
    }

    fn version(&self) -> String {
        "canned".into()
    }

    fn probe(&self, device: Device) -> bool {
        device == Device::Cpu
    }

    fn load_generator(&self, _spec: &LoadSpec<'_>) -> Result<Arc<dyn Generate>, InferError> {
        Ok(self.0.clone())
    }
}

fn tasks_replying(reply: &str) -> (RoutedTasks, Arc<CannedGenerator>) {
    let generator = Arc::new(CannedGenerator::default());
    *generator.reply.lock().unwrap() = reply.to_owned();
    let router = Router::new(
        Registry::from_ron(REGISTRY).unwrap(),
        vec![Arc::new(CannedRuntime(generator.clone()))],
        RouterSettings::new("/nonexistent/models".into()),
    );
    (RoutedTasks::new(Arc::new(router)), generator)
}

fn job() -> Job {
    Job::within(Duration::from_secs(2), Priority::Interactive)
}

fn screen() -> RgbImage {
    RgbImage::new(2, 2, 6, vec![0; 12].into()).unwrap()
}

#[test]
fn describe_screen_sends_raw_pixels_and_a_schema() {
    let (tasks, generator) = tasks_replying(
        r#"{"content_kind": "code", "caption": "Edits the router.", "visible_entities": ["router.rs"]}"#,
    );
    let description = tasks
        .describe_screen(&screen(), "Zed", "router.rs", job())
        .unwrap();
    assert_eq!(description.content_kind, ContentKind::Code);
    assert_eq!(description.visible_entities, ["router.rs"]);

    let request = generator.last_request.lock().unwrap().clone().unwrap();
    assert!(matches!(request.media.as_slice(), [crate::Media::Rgb8(_)]));
    let Output::Json(schema) = request.output else {
        panic!("no schema");
    };
    let schema = serde_json::to_string(schema.as_ref()).unwrap();
    assert!(schema.contains("visible_entities") && !schema.contains("$ref"));
}

#[test]
fn output_that_does_not_parse_is_a_schema_error() {
    let (tasks, _) = tasks_replying("not json");
    assert!(matches!(
        tasks.describe_screen(&screen(), "Zed", "router.rs", job()),
        Err(InferError::Schema(_))
    ));
}

#[test]
fn summaries_must_match_the_segments_one_to_one() {
    let summary = r#"{"summary": "s", "activity_label": "coding", "entities": [], "facts": [], "open_items": []}"#;
    let (tasks, _) = tasks_replying(&format!(r#"{{"segments": [{summary}]}}"#));
    let segment = SegmentDigest {
        app: "Zed".into(),
        title: "router.rs".into(),
        ..SegmentDigest::default()
    };
    assert_eq!(
        tasks
            .summarize_segments(std::slice::from_ref(&segment), job())
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        tasks.summarize_segments(&[segment.clone(), segment], job()),
        Err(InferError::Schema(_))
    ));
}
