use std::time::Duration;

use context_infer::registry::ModelEntry;
use context_infer::{Embed, EmbedPurpose, EmbedSpace, Embeddings, InferError, Job, Priority};
use serde::{Deserialize, Serialize};

use crate::backend::RemoteModel;
use crate::response::post_json;

/// The embedding trait has no deadline; a remote call gets this much.
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct RemoteEmbedder {
    model: RemoteModel,
    space: EmbedSpace,
    query_prefix: String,
    document_prefix: String,
}

impl RemoteEmbedder {
    pub(crate) fn new(model: RemoteModel, space: EmbedSpace, entry: &ModelEntry) -> Self {
        Self {
            model,
            space,
            query_prefix: entry.query_prefix.clone(),
            document_prefix: entry.document_prefix.clone(),
        }
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingRow>,
}

#[derive(Deserialize)]
struct EmbeddingRow {
    index: usize,
    embedding: Vec<f32>,
}

impl Embed for RemoteEmbedder {
    fn space(&self) -> &EmbedSpace {
        &self.space
    }

    fn embed(&self, texts: &[String], purpose: EmbedPurpose) -> Result<Embeddings, InferError> {
        let prefix = match purpose {
            EmbedPurpose::Query => &self.query_prefix,
            EmbedPurpose::Document => &self.document_prefix,
        };
        let body = EmbeddingRequest {
            model: &self.model.name,
            input: texts.iter().map(|text| format!("{prefix}{text}")).collect(),
        };
        let deadline = Job::within(EMBED_TIMEOUT, Priority::Background).deadline;
        let mut answer: EmbeddingResponse = post_json(
            &self.model.agent,
            self.model.credentials.as_ref(),
            &self.model.url,
            &body,
            deadline,
        )?;
        if answer.data.len() != texts.len() {
            return Err(InferError::Backend(format!(
                "{} vectors for {} texts",
                answer.data.len(),
                texts.len()
            )));
        }
        answer.data.sort_by_key(|row| row.index);
        let mut embeddings = Embeddings {
            dimensions: self.space.dimensions as usize,
            values: Vec::with_capacity(texts.len() * self.space.dimensions as usize),
        };
        for row in &answer.data {
            embeddings.push_normalized(&row.embedding);
        }
        Ok(embeddings)
    }
}
