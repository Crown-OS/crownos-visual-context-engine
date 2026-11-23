//! Runs real models. Set `$CROWN_CONTEXT_MODELS` to a directory laid out as
//! `models/registry.ron` names the files, then `cargo test -- --ignored`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use context_infer::{
    EmbedPurpose, Image, Job, PixelFormat, Priority, Registry, Router, RouterSettings,
    RuntimeBackend,
};
use context_infer_ort::OrtBackend;

fn router() -> Router {
    let models =
        PathBuf::from(std::env::var_os("CROWN_CONTEXT_MODELS").expect("CROWN_CONTEXT_MODELS"));
    let registry = Registry::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/registry.ron"),
        None,
    )
    .unwrap();
    let backends: Vec<Arc<dyn RuntimeBackend>> = vec![Arc::new(OrtBackend::new())];
    Router::new(registry, backends, RouterSettings::new(models))
}

fn job() -> Job {
    Job::within(Duration::from_secs(60), Priority::Interactive)
}

#[test]
#[ignore = "needs ONNX Runtime and the models in $CROWN_CONTEXT_MODELS"]
fn ocr_runs_every_tile_of_a_blank_screen() {
    let blank = Image::new(
        1280,
        720,
        1280 * 4,
        PixelFormat::Xrgb8888,
        vec![255; 1280 * 720 * 4].into(),
    )
    .unwrap();
    assert!(router().recognize_text(blank, &job()).unwrap().is_empty());
}

#[test]
#[ignore = "needs ONNX Runtime and the models in $CROWN_CONTEXT_MODELS"]
fn similar_texts_embed_close_together() {
    let texts = [
        "open the display settings",
        "change the screen brightness",
        "a recipe for banana bread",
    ];
    let embedded = router()
        .embed(
            texts.iter().map(|text| (*text).to_owned()).collect(),
            EmbedPurpose::Document,
            &job(),
        )
        .unwrap();
    let rows: Vec<&[f32]> = embedded.embeddings.rows().collect();
    let cosine =
        |left: &[f32], right: &[f32]| left.iter().zip(right).map(|(l, r)| l * r).sum::<f32>();
    assert!(cosine(rows[0], rows[1]) > cosine(rows[0], rows[2]));
}
