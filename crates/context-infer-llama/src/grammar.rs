use context_infer::{InferError, Output, Sampling};
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;

use crate::util::error::backend_error;

/// The sampler chain for `output`: a JSON-schema grammar first when there is one, then
/// nucleus sampling, or greedy picks at temperature zero.
pub(crate) fn sampler(
    model: &LlamaModel,
    output: &Output,
    sampling: Sampling,
) -> Result<LlamaSampler, InferError> {
    let mut chain = Vec::new();
    if let Output::Json(schema) = output {
        let schema = serde_json::to_string(schema.as_ref())
            .map_err(|error| InferError::Schema(error.to_string()))?;
        let grammar = llama_cpp_2::json_schema_to_grammar(&schema)
            .map_err(|error| InferError::Schema(error.to_string()))?;
        chain.push(
            LlamaSampler::grammar(model, &grammar, "root")
                .map_err(|error| backend_error("building the grammar", error))?,
        );
    }
    if sampling.temperature <= 0.0 {
        chain.push(LlamaSampler::greedy());
    } else {
        chain.extend([
            LlamaSampler::top_p(sampling.top_p, 1),
            LlamaSampler::temp(sampling.temperature),
            LlamaSampler::dist(sampling.seed),
        ]);
    }
    Ok(LlamaSampler::chain_simple(chain))
}
