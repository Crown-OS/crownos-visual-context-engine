use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Code,
    Terminal,
    Document,
    WebPage,
    Chat,
    Email,
    Spreadsheet,
    Design,
    Video,
    Image,
    Game,
    Settings,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenDescription {
    pub content_kind: ContentKind,
    /// One sentence.
    pub caption: String,
    /// Named things in view: projects, files, people, products, sites.
    pub visible_entities: Vec<String>,
}

pub(crate) const DESCRIBE_SCREEN_SYSTEM_PROMPT: &str = "You describe a screenshot of one \
application window for a private activity log. Answer with JSON only. caption is one short \
sentence about what the user works on. visible_entities lists at most 8 named things in view. \
Do not copy passwords, keys, tokens or other secrets.";

pub(crate) fn describe_screen_prompt(app_name: &str, window_title: &str) -> String {
    format!("Application: {app_name}\nWindow title: {window_title}\nDescribe the screenshot.")
}
