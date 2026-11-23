use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the engine knows about one closed segment, already free of secrets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegmentDigest {
    pub app: String,
    pub title: String,
    /// The URL or file path, when known.
    pub location: Option<String>,
    pub label_candidates: Vec<String>,
    /// Deduplicated visible text, already cut to the prompt budget.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Entity {
    pub name: String,
    /// For example person, project, file, site, product or organisation.
    pub kind: String,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Fact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SegmentSummary {
    pub summary: String,
    pub activity_label: String,
    pub entities: Vec<Entity>,
    pub facts: Vec<Fact>,
    pub open_items: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SegmentSummaries {
    pub(crate) segments: Vec<SegmentSummary>,
}

pub(crate) const SUMMARIZE_SYSTEM_PROMPT: &str = "You summarize segments of computer activity \
for a private log. Answer with JSON only, one entry in segments for each input segment, in \
input order. activity_label is one of the label candidates when one fits. Keep facts that \
stay true after today. Do not copy passwords, keys, tokens or other secrets.";

/// The user message of `summarize_segments`: one block per segment.
pub fn summarize_prompt(segments: &[SegmentDigest]) -> String {
    let mut prompt = String::new();
    for (number, segment) in segments.iter().enumerate() {
        let _ = writeln!(prompt, "## Segment {}", number + 1);
        let _ = writeln!(prompt, "App: {}\nTitle: {}", segment.app, segment.title);
        if let Some(location) = &segment.location {
            let _ = writeln!(prompt, "Location: {location}");
        }
        let _ = writeln!(
            prompt,
            "Label candidates: {}\nText:\n{}\n",
            segment.label_candidates.join(", "),
            segment.text
        );
    }
    prompt
}
