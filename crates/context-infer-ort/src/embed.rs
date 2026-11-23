use std::borrow::Cow;

use context_infer::registry::{Artifact, Pooling};
use context_infer::{Embed, EmbedPurpose, EmbedSpace, Embeddings, InferError, LoadSpec};
use ort::environment::Environment;
use ort::session::SessionInputValue;
use ort::value::Tensor;
use tokenizers::{Tokenizer, TruncationParams};

use crate::session::GraphSession;
use crate::util::error::backend_error;

const MAX_TOKENS: usize = 512;
const POOLED_OUTPUT: &str = "sentence_embedding";

/// A text encoder graph with its tokenizer. Pools token states as the registry says, cuts
/// the vector to the registry's dimensions (Matryoshka) and normalises it.
#[derive(Debug)]
pub struct OnnxEmbedder {
    graph: GraphSession,
    tokenizer: Tokenizer,
    space: EmbedSpace,
    pooling: Pooling,
    query_prefix: String,
    document_prefix: String,
}

impl OnnxEmbedder {
    /// Needs the graph `encoder` and the file `tokenizer` in the ONNX artifact.
    pub fn load(environment: &Environment, spec: &LoadSpec<'_>) -> Result<Self, InferError> {
        let unavailable =
            |what: &str| InferError::Unavailable(format!("{what} of {}", spec.model_id).into());
        let Artifact::Onnx { graphs, files, .. } = spec.artifact else {
            return Err(unavailable("an ONNX artifact"));
        };
        let encoder = graphs
            .get("encoder")
            .ok_or_else(|| unavailable("the encoder graph"))?;
        let tokenizer_file = files
            .get("tokenizer")
            .ok_or_else(|| unavailable("the tokenizer"))?;
        let mut tokenizer = Tokenizer::from_file(spec.models_dir.join(&tokenizer_file.path))
            .map_err(|error| backend_error("reading the tokenizer", error))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..TruncationParams::default()
            }))
            .map_err(|error| backend_error("setting the truncation", error))?;
        Ok(Self {
            graph: GraphSession::open(
                environment,
                &spec.models_dir.join(&encoder.path),
                spec.device,
            )?,
            tokenizer,
            space: spec
                .embed_space()
                .ok_or_else(|| unavailable("the embedding dimensions"))?,
            pooling: spec.model.pooling.unwrap_or(Pooling::Mean),
            query_prefix: spec.model.query_prefix.clone(),
            document_prefix: spec.model.document_prefix.clone(),
        })
    }

    fn token_batch(
        &self,
        texts: &[String],
        purpose: EmbedPurpose,
    ) -> Result<TokenBatch, InferError> {
        let prefix = match purpose {
            EmbedPurpose::Query => &self.query_prefix,
            EmbedPurpose::Document => &self.document_prefix,
        };
        let inputs: Vec<String> = texts.iter().map(|text| format!("{prefix}{text}")).collect();
        let encodings = self
            .tokenizer
            .encode_batch(inputs, true)
            .map_err(|error| backend_error("tokenizing", error))?;
        let length = encodings
            .iter()
            .map(|encoding| encoding.get_ids().len())
            .max()
            .unwrap_or(0)
            .max(1);
        let mut batch = TokenBatch {
            rows: encodings.len(),
            length,
            ids: vec![0; encodings.len() * length],
            mask: vec![0; encodings.len() * length],
        };
        for ((ids, mask), encoding) in batch
            .ids
            .chunks_exact_mut(length)
            .zip(batch.mask.chunks_exact_mut(length))
            .zip(&encodings)
        {
            for ((id, flag), (&token, &attention)) in ids
                .iter_mut()
                .zip(mask.iter_mut())
                .zip(encoding.get_ids().iter().zip(encoding.get_attention_mask()))
            {
                *id = i64::from(token);
                *flag = i64::from(attention);
            }
        }
        Ok(batch)
    }
}

/// Right-padded token ids and attention mask, `rows` x `length`.
struct TokenBatch {
    rows: usize,
    length: usize,
    ids: Vec<i64>,
    mask: Vec<i64>,
}

impl TokenBatch {
    fn input(&self, name: &str) -> Result<Option<SessionInputValue<'static>>, InferError> {
        let shape = [self.rows, self.length];
        let values: Cow<'_, [i64]> = match name {
            "input_ids" => Cow::Borrowed(&self.ids),
            "attention_mask" => Cow::Borrowed(&self.mask),
            "token_type_ids" => Cow::Owned(vec![0; self.ids.len()]),
            "position_ids" => {
                Cow::Owned((0..self.rows).flat_map(|_| 0..self.length as i64).collect())
            }
            _ => return Ok(None),
        };
        let tensor = Tensor::from_array((shape, values.into_owned()))
            .map_err(|error| backend_error("building the token tensor", error))?;
        Ok(Some(tensor.into()))
    }
}

impl Embed for OnnxEmbedder {
    fn space(&self) -> &EmbedSpace {
        &self.space
    }

    fn embed(&self, texts: &[String], purpose: EmbedPurpose) -> Result<Embeddings, InferError> {
        let dimensions = self.space.dimensions as usize;
        if texts.is_empty() {
            return Ok(Embeddings {
                dimensions,
                values: Vec::new(),
            });
        }
        let batch = self.token_batch(texts, purpose)?;
        let mut inputs = Vec::new();
        for name in self.graph.input_names() {
            let value = batch.input(name)?.ok_or_else(|| {
                InferError::Backend(format!("the encoder wants an unknown input {name}"))
            })?;
            inputs.push((Cow::Owned(name.clone()), value));
        }
        let pooled_output = self
            .graph
            .output_names()
            .iter()
            .any(|name| name == POOLED_OUTPUT);
        let output = if pooled_output {
            POOLED_OUTPUT
        } else {
            self.graph.first_output()?
        };
        let mut session = self.graph.lock();
        let outputs = session
            .run(inputs)
            .map_err(|error| backend_error("running the encoder", error))?;
        let (shape, states) = outputs
            .get(output)
            .ok_or_else(|| InferError::Backend(format!("no output {output}")))?
            .try_extract_tensor::<f32>()
            .map_err(|error| backend_error("reading the embeddings", error))?;
        let width = shape
            .last()
            .and_then(|&width| usize::try_from(width).ok())
            .filter(|&width| width >= dimensions)
            .ok_or_else(|| {
                InferError::Backend(format!(
                    "the encoder gives {shape:?}, not {dimensions} wide"
                ))
            })?;
        let mut embeddings = Embeddings {
            dimensions,
            values: Vec::with_capacity(batch.rows * dimensions),
        };
        for row in 0..batch.rows {
            let vector = if pooled_output {
                states
                    .get(row * width..(row + 1) * width)
                    .map(<[f32]>::to_vec)
                    .unwrap_or_default()
            } else {
                let mask = batch.mask.get(row * batch.length..(row + 1) * batch.length);
                let states =
                    states.get(row * batch.length * width..(row + 1) * batch.length * width);
                pool(
                    states.unwrap_or_default(),
                    mask.unwrap_or_default(),
                    width,
                    self.pooling,
                )
            };
            embeddings.push_normalized(&vector);
        }
        Ok(embeddings)
    }
}

/// One vector from the token states of one text; `mask` marks the real tokens.
fn pool(states: &[f32], mask: &[i64], width: usize, pooling: Pooling) -> Vec<f32> {
    let tokens = states.chunks_exact(width.max(1));
    let real: Vec<&[f32]> = tokens
        .zip(mask)
        .filter(|&(_, &flag)| flag != 0)
        .map(|(token, _)| token)
        .collect();
    match pooling {
        Pooling::Cls => real.first().map(|token| token.to_vec()).unwrap_or_default(),
        Pooling::LastToken => real.last().map(|token| token.to_vec()).unwrap_or_default(),
        Pooling::Mean => {
            let mut sum = vec![0.0; width];
            for token in &real {
                for (total, value) in sum.iter_mut().zip(token.iter()) {
                    *total += value;
                }
            }
            let count = real.len().max(1) as f32;
            sum.iter().map(|total| total / count).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [f32; 6] = [1.0, 0.0, 3.0, 4.0, 9.0, 9.0];

    #[test]
    fn pools_only_real_tokens() {
        let mask = [1, 1, 0];
        assert_eq!(pool(&STATES, &mask, 2, Pooling::Cls), [1.0, 0.0]);
        assert_eq!(pool(&STATES, &mask, 2, Pooling::LastToken), [3.0, 4.0]);
        assert_eq!(pool(&STATES, &mask, 2, Pooling::Mean), [2.0, 2.0]);
    }
}
