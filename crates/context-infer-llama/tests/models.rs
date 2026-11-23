//! The first test needs no model. The ignored ones run real models: set
//! `$CROWN_CONTEXT_MODELS` to a directory laid out as `models/registry.ron` names the files,
//! then `cargo test -- --ignored`. `$CROWN_CONTEXT_MODELS_OVERRIDE` names a user registry
//! file to put on top, for other models.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use context_infer::{
    ContextTasks, Device, EmbedPurpose, Image, Job, PixelFormat, Priority, Registry, RoutedTasks,
    Router, RouterSettings, RuntimeBackend, SegmentDigest,
};
use context_infer_llama::LlamaBackendHost;

/// llama.cpp starts once per process.
static HOST: LazyLock<Arc<LlamaBackendHost>> =
    LazyLock::new(|| Arc::new(LlamaBackendHost::new(None).expect("llama.cpp starts")));

fn router() -> Arc<Router> {
    let models =
        PathBuf::from(std::env::var_os("CROWN_CONTEXT_MODELS").expect("CROWN_CONTEXT_MODELS"));
    let user_override = std::env::var_os("CROWN_CONTEXT_MODELS_OVERRIDE").map(PathBuf::from);
    let registry = Registry::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/registry.ron"),
        user_override.as_deref(),
    )
    .unwrap();
    let backends: Vec<Arc<dyn RuntimeBackend>> = vec![HOST.clone()];
    Arc::new(Router::new(registry, backends, RouterSettings::new(models)))
}

fn job() -> Job {
    Job::within(Duration::from_secs(300), Priority::Interactive)
}

#[test]
fn the_cpu_module_loads_without_a_model() {
    assert!(HOST.probe(Device::Cpu));
    assert!(!HOST.probe(Device::Remote));
    assert!(HOST.version().contains("CPU"));
}

#[test]
#[ignore = "needs the models in $CROWN_CONTEXT_MODELS"]
fn describes_a_screen_as_schema_json() {
    let (width, height) = (640, 360);
    let screen = Image::new(
        width,
        height,
        width as usize * 4,
        PixelFormat::Xrgb8888,
        vec![200; width as usize * height as usize * 4].into(),
    )
    .unwrap();
    let description = RoutedTasks::new(router())
        .describe_screen(&screen, "Files", "Home", job())
        .unwrap();
    assert!(!description.caption.is_empty());
}

#[test]
#[ignore = "needs the models in $CROWN_CONTEXT_MODELS"]
fn embeds_with_a_gguf_model() {
    let embedded = router()
        .embed(vec!["display settings".into()], EmbedPurpose::Query, &job())
        .unwrap();
    let length: f32 = embedded
        .embeddings
        .values
        .iter()
        .map(|value| value * value)
        .sum();
    assert!((length - 1.0).abs() < 1e-3);
}

#[test]
#[ignore = "needs the models in $CROWN_CONTEXT_MODELS"]
fn summarizes_segments_as_schema_json() {
    let segment = SegmentDigest {
        app: "Zed".into(),
        title: "router.rs - crownos-visual-context-engine".into(),
        location: Some("crates/context-infer/src/router/mod.rs".into()),
        label_candidates: vec!["coding".into(), "code_review".into()],
        text: "fn ranked(&self, task: Task) -> Vec<Candidate> { ... circuit breaker ... }".into(),
    };
    let summaries = RoutedTasks::new(router())
        .summarize_segments(&[segment], job())
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(!summaries[0].summary.is_empty());
}
