use crate::error::InferError;

pub trait Rerank: Send + Sync {
    /// One relevance score per document, in input order; higher is more relevant.
    fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, InferError>;
}
