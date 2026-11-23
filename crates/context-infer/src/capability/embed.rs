use std::fmt;

use crownos_config::system::ai::Runtime;
use serde::{Deserialize, Serialize};

use crate::error::InferError;

/// The vector space of one build of an embedding model. Vectors from different spaces do
/// not compare, so a store that sees a new space must embed its content again. Two builds
/// of the same model (int8 ONNX, Q8 GGUF) are different spaces.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmbedSpace {
    pub model: String,
    pub runtime: Runtime,
    pub quantization: String,
    pub dimensions: u32,
}

impl fmt::Display for EmbedSpace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}/{:?}/{}/{}",
            self.model, self.runtime, self.quantization, self.dimensions
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedPurpose {
    Query,
    Document,
}

/// Row-major vectors, `dimensions` values each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Embeddings {
    pub dimensions: usize,
    pub values: Vec<f32>,
}

impl Embeddings {
    pub fn rows(&self) -> impl Iterator<Item = &[f32]> {
        self.values.chunks_exact(self.dimensions.max(1))
    }

    pub fn len(&self) -> usize {
        self.values.len().checked_div(self.dimensions).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds the first `dimensions` values of `vector` (a Matryoshka cut), scaled to unit
    /// length.
    pub fn push_normalized(&mut self, vector: &[f32]) {
        let truncated = vector.get(..self.dimensions).unwrap_or(vector);
        let length = truncated
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        let scale = if length > 0.0 { 1.0 / length } else { 0.0 };
        self.values
            .extend(truncated.iter().map(|value| value * scale));
    }
}

pub trait Embed: Send + Sync {
    /// The space from the registry entry the backend was loaded from.
    fn space(&self) -> &EmbedSpace;

    /// One normalised vector per text, in input order.
    fn embed(&self, texts: &[String], purpose: EmbedPurpose) -> Result<Embeddings, InferError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pushed_vector_is_cut_and_has_unit_length() {
        let mut embeddings = Embeddings {
            dimensions: 2,
            values: Vec::new(),
        };
        embeddings.push_normalized(&[3.0, 4.0, 12.0]);
        embeddings.push_normalized(&[0.0, 0.0]);
        assert_eq!(embeddings.values, [0.6, 0.8, 0.0, 0.0]);
    }
}
