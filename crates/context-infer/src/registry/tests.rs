#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use crownos_config::system::ai::{Device, Runtime};

use super::{Artifact, Registry, RegistryError};
use crate::task::Task;

const SHIPPED: &str = include_str!("../../../../models/registry.ron");

const MINIMAL: &str = r#"(
    version: 2,
    models: {
        "ocr": (capabilities: [Ocr], licence: "Apache-2.0",
                artifacts: [Onnx(graphs: {"det": (path: "det.onnx"), "rec": (path: "rec.onnx")},
                                 files: {"dict": (path: "dict.txt")})]),
    },
    tasks: {
        RecognizeText: [(model: "ocr", runtime: OnnxRuntime, device: Cpu)],
    },
)"#;

fn scratch_file(name: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "context-infer-registry-{}-{name}",
        std::process::id()
    ));
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn the_shipped_registry_is_valid() {
    let registry = Registry::from_ron(SHIPPED).unwrap();
    for task in [
        Task::RecognizeText,
        Task::EmbedText,
        Task::SummarizeSegments,
        Task::DescribeScreen,
    ] {
        assert!(!registry.candidates(task).is_empty(), "{task:?}");
    }
    let ocr = &registry.models["pp-ocrv6-det-tiny-rec-small"];
    let Some(Artifact::Onnx { graphs, files, .. }) = ocr.artifact_for(Runtime::OnnxRuntime) else {
        panic!("no ONNX artifact");
    };
    assert!(graphs.contains_key("det") && graphs.contains_key("rec"));
    assert!(files.contains_key("dict"));
    assert!(
        registry
            .models
            .values()
            .all(|model| !model.licence.is_empty())
    );
}

#[test]
fn refuses_a_candidate_with_an_unknown_model() {
    let text = MINIMAL.replace(r#"(model: "ocr","#, r#"(model: "missing","#);
    assert!(matches!(
        Registry::from_ron(&text),
        Err(RegistryError::UnknownModel { .. })
    ));
}

#[test]
fn refuses_a_device_the_runtime_cannot_use() {
    let text = MINIMAL.replace("device: Cpu", "device: Vulkan");
    assert!(matches!(
        Registry::from_ron(&text),
        Err(RegistryError::UnsupportedDevice {
            runtime: Runtime::OnnxRuntime,
            device: Device::Vulkan
        })
    ));
}

#[test]
fn refuses_a_model_without_the_capability_the_task_needs() {
    let text = MINIMAL.replace("RecognizeText:", "EmbedText:");
    assert!(matches!(
        Registry::from_ron(&text),
        Err(RegistryError::MissingCapability { .. })
    ));
}

#[test]
fn refuses_the_first_schema() {
    let text = MINIMAL.replace("version: 2", "version: 1");
    assert!(matches!(
        Registry::from_ron(&text),
        Err(RegistryError::Version { found: 1 })
    ));
}

#[test]
fn the_user_file_replaces_models_and_task_lists_by_name() {
    let system = scratch_file("system.ron", MINIMAL);
    let user = scratch_file(
        "user.ron",
        r#"(
            models: {
                "my-ocr": (capabilities: [Ocr], licence: "MIT",
                           artifacts: [Onnx(graphs: {"det": (path: "mine.onnx")})]),
            },
            tasks: {
                RecognizeText: [(model: "my-ocr", runtime: OnnxRuntime, device: OpenVinoNpu)],
            },
        )"#,
    );
    let registry = Registry::load(&system, Some(&user)).unwrap();
    assert_eq!(registry.candidates(Task::RecognizeText)[0].model, "my-ocr");
    assert!(registry.models.contains_key("ocr"));

    let missing_user_file = system.with_extension("absent");
    let registry = Registry::load(&system, Some(&missing_user_file)).unwrap();
    assert_eq!(registry.candidates(Task::RecognizeText)[0].model, "ocr");
    remove_scratch_files(&[system, user]);
}

fn remove_scratch_files(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_broken_user_file_falls_back_to_the_system_registry() {
    let system = scratch_file("system-for-broken.ron", MINIMAL);
    for (name, contents) in [
        ("syntax.ron", "(tasks: {RecognizeText: [("),
        (
            "field.ron",
            r#"(tasks: {RecognizeText: [(model: "ocr", runtime: OnnxRuntime, device: Cpu, typo: 1)]})"#,
        ),
        (
            "unknown-model.ron",
            r#"(tasks: {RecognizeText: [(model: "nope", runtime: OnnxRuntime, device: Cpu)]})"#,
        ),
    ] {
        let user = scratch_file(name, contents);
        let registry = Registry::load(&system, Some(&user)).unwrap();
        assert_eq!(
            registry.candidates(Task::RecognizeText)[0].model,
            "ocr",
            "{name}"
        );
        remove_scratch_files(&[user]);
    }
    remove_scratch_files(&[system]);
}
