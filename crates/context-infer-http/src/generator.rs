use std::ops::ControlFlow;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use context_infer::{
    GenRequest, Generate, GenerateCapabilities, Generation, InferError, Media, Output, Role,
};
use serde::{Deserialize, Serialize};

use crate::backend::RemoteModel;
use crate::response::post_json;
use crate::util::png::encode_png;

#[derive(Debug)]
pub(crate) struct RemoteGenerator {
    model: RemoteModel,
}

impl RemoteGenerator {
    pub(crate) const fn new(model: RemoteModel) -> Self {
        Self { model }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    max_tokens: u32,
    temperature: f32,
    top_p: f32,
    seed: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat<'a>>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'static str,
    content: Vec<ContentPart<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart<'a> {
    Text { text: &'a str },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Serialize)]
struct ResponseFormat<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    json_schema: NamedSchema<'a>,
}

#[derive(Serialize)]
struct NamedSchema<'a> {
    name: &'static str,
    schema: &'a serde_json::Value,
    strict: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: AnswerMessage,
}

#[derive(Deserialize)]
struct AnswerMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

/// `data:image/png;base64,...` for each image; encoded media keep their own type.
fn image_urls(media: &[Media]) -> Result<Vec<ImageUrl>, InferError> {
    media
        .iter()
        .map(|media| {
            let (mime, bytes) = match media {
                Media::Image(image) => ("image/png".into(), encode_png(image)?),
                Media::Encoded { mime, bytes } => (mime.to_string(), bytes.as_slice().to_vec()),
                Media::Pcm16k(_) => return Err(InferError::Unavailable("remote audio".into())),
            };
            Ok(ImageUrl {
                url: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
            })
        })
        .collect()
}

impl Generate for RemoteGenerator {
    fn capabilities(&self) -> GenerateCapabilities {
        GenerateCapabilities {
            images: true,
            json_schema: true,
            context_tokens: 32_768,
        }
    }

    /// One request; the whole answer goes to `on_text` at the end.
    fn stream(
        &self,
        request: &GenRequest,
        on_text: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<Generation, InferError> {
        let schema = match &request.output {
            Output::Json(schema) => Some(schema.as_value()),
            Output::Text => None,
        };
        let mut images = image_urls(&request.media)?.into_iter();
        let last_user = request
            .messages
            .iter()
            .rposition(|message| message.role == Role::User);
        let messages = request
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let mut content = vec![ContentPart::Text {
                    text: &message.content,
                }];
                if Some(index) == last_user {
                    content.extend(
                        images
                            .by_ref()
                            .map(|image_url| ContentPart::ImageUrl { image_url }),
                    );
                }
                ChatMessage {
                    role: match message.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    },
                    content,
                }
            })
            .collect();
        let body = ChatRequest {
            model: &self.model.name,
            messages,
            max_tokens: request.max_tokens,
            temperature: request.sampling.temperature,
            top_p: request.sampling.top_p,
            seed: request.sampling.seed,
            response_format: schema.map(|schema| ResponseFormat {
                kind: "json_schema",
                json_schema: NamedSchema {
                    name: "output",
                    schema,
                    strict: true,
                },
            }),
        };
        let answer: ChatResponse = post_json(
            &self.model.agent,
            self.model.credentials.as_ref(),
            &self.model.url,
            &body,
            request.job.deadline,
        )?;
        let text = answer
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or_else(|| InferError::Backend("the server gave no answer".into()))?;
        let _ = on_text(&text);
        let usage = answer.usage.unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
        });
        Ok(Generation {
            text,
            prompt_tokens: usage.prompt_tokens,
            generated_tokens: usage.completion_tokens,
        })
    }
}
