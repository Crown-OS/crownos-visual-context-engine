//! PP-OCR on ONNX Runtime: a DB text detector on fixed-size tiles at native resolution, then
//! a CTC recogniser on text lines batched by width bucket. Fixed shapes keep NPU and TensorRT
//! graphs compiled once.

mod components;
mod ctc;
mod detect;
mod recognize;
mod tensor;

use std::path::Path;

use context_infer::registry::{Artifact, OnnxGraph};
use context_infer::{Image, InferError, LoadSpec, Ocr, TextLine};
use ort::environment::Environment;

use crate::session::GraphSession;
use detect::TextDetector;
use recognize::TextRecognizer;

#[derive(Debug)]
pub struct PaddleOcr {
    detector: TextDetector,
    recognizer: TextRecognizer,
}

impl PaddleOcr {
    /// Needs the graphs `det` and `rec` and the file `dict` in the ONNX artifact.
    pub fn load(environment: &Environment, spec: &LoadSpec<'_>) -> Result<Self, InferError> {
        let Artifact::Onnx { graphs, files, .. } = spec.artifact else {
            return Err(InferError::Unavailable("an ONNX artifact for OCR".into()));
        };
        let graph = |role: &str| -> Result<&OnnxGraph, InferError> {
            graphs.get(role).ok_or_else(|| {
                InferError::Unavailable(format!("the {role} graph of {}", spec.model_id).into())
            })
        };
        let open = |graph: &OnnxGraph| {
            GraphSession::open(environment, &spec.models_dir.join(&graph.path), spec.device)
        };
        let (det, rec) = (graph("det")?, graph("rec")?);
        let dictionary = files.get("dict").ok_or_else(|| {
            InferError::Unavailable(format!("the dict file of {}", spec.model_id).into())
        })?;
        Ok(Self {
            detector: TextDetector::new(open(det)?, &det.shapes),
            recognizer: TextRecognizer::new(
                open(rec)?,
                &rec.shapes,
                &read_dictionary(&spec.models_dir.join(&dictionary.path))?,
            ),
        })
    }
}

impl Ocr for PaddleOcr {
    fn recognize(&self, image: &Image) -> Result<Vec<TextLine>, InferError> {
        let boxes = self.detector.detect(image)?;
        self.recognizer.recognize(image, &boxes)
    }
}

fn read_dictionary(path: &Path) -> Result<String, InferError> {
    std::fs::read_to_string(path).map_err(|error| {
        InferError::Unavailable(format!("the OCR dictionary {} ({error})", path.display()).into())
    })
}
