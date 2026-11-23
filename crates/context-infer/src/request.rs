use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use schemars::{JsonSchema, Schema};

use crate::error::InferError;
use crate::media::Media;
use crate::util::json_schema::schema_for;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub role: Role,
    pub content: Cow<'static, str>,
}

impl Message {
    pub fn system(content: impl Into<Cow<'static, str>>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<Cow<'static, str>>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
}

/// The shape of the generated text. A backend enforces a schema with a grammar.
#[derive(Clone, Debug)]
pub enum Output {
    Text,
    Json(Arc<Schema>),
}

impl Output {
    pub fn json_for<T: JsonSchema>() -> Self {
        Self::Json(Arc::new(schema_for::<T>()))
    }

    /// The schema as JSON text, for another process; `None` for plain text.
    pub fn schema_text(&self) -> Option<String> {
        match self {
            Self::Text => None,
            Self::Json(schema) => serde_json::to_string(schema.as_value()).ok(),
        }
    }

    /// A schema that another process sent as JSON text.
    pub fn json_from_text(schema: &str) -> Result<Self, InferError> {
        let invalid = |error: String| InferError::InvalidInput(format!("the schema: {error}"));
        let value: serde_json::Value =
            serde_json::from_str(schema).map_err(|error| invalid(error.to_string()))?;
        let schema = Schema::try_from(value).map_err(|error| invalid(error.to_string()))?;
        Ok(Self::Json(Arc::new(schema)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampling {
    pub temperature: f32,
    pub top_p: f32,
    pub seed: u32,
}

impl Default for Sampling {
    fn default() -> Self {
        Self {
            temperature: 0.2,
            top_p: 0.9,
            seed: 42,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    #[default]
    Background,
    Interactive,
}

/// When a call must finish, how urgent it is, and what can end it sooner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub deadline: Instant,
    pub priority: Priority,
    pub cancel: JobCancel,
}

impl Job {
    pub fn within(budget: Duration, priority: Priority) -> Self {
        Self {
            deadline: Instant::now() + budget,
            priority,
            cancel: JobCancel::default(),
        }
    }
}

/// Ends a job before its deadline: a job that has not reached its model does not run, and the
/// caller stops waiting for one that has.
#[derive(Clone, Debug, Default)]
pub struct JobCancel(Arc<AtomicBool>);

impl JobCancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl PartialEq for JobCancel {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for JobCancel {}

#[derive(Clone, Debug)]
pub struct GenRequest {
    pub messages: Vec<Message>,
    pub media: Vec<Media>,
    pub output: Output,
    pub sampling: Sampling,
    pub max_tokens: u32,
    pub job: Job,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Generation {
    pub text: String,
    pub prompt_tokens: u32,
    pub generated_tokens: u32,
}
