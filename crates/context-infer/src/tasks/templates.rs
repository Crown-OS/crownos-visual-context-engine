use std::collections::BTreeMap;

use super::describe::DESCRIBE_SCREEN_SYSTEM_PROMPT;
use super::summarize::SUMMARIZE_SYSTEM_PROMPT;
use crate::error::InferError;
use crate::request::Message;
use crate::task::Task;

/// A prompt that another process names by id and fills with `{name}` variables, so prompts
/// stay in the model process and callers send only data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptTemplate {
    pub id: &'static str,
    pub task: Task,
    pub system: &'static str,
    pub user: &'static str,
}

pub const PROMPT_TEMPLATES: &[PromptTemplate] = &[
    PromptTemplate {
        id: "summarize_segments",
        task: Task::SummarizeSegments,
        system: SUMMARIZE_SYSTEM_PROMPT,
        user: "{segments}",
    },
    PromptTemplate {
        id: "describe_screen",
        task: Task::DescribeScreen,
        system: DESCRIBE_SCREEN_SYSTEM_PROMPT,
        user: "Application: {app}\nWindow title: {title}\nDescribe the screenshot.",
    },
];

impl PromptTemplate {
    pub fn find(id: &str) -> Result<&'static Self, InferError> {
        PROMPT_TEMPLATES
            .iter()
            .find(|template| template.id == id)
            .ok_or_else(|| InferError::Unavailable(format!("the prompt template {id}").into()))
    }

    /// The system and user messages with each `{name}` replaced by its variable. Variable
    /// values are not scanned again, so they may hold braces.
    pub fn render(&self, variables: &BTreeMap<String, String>) -> Result<Vec<Message>, InferError> {
        let mut user = String::with_capacity(self.user.len());
        let mut rest = self.user;
        while let Some(open) = rest.find('{') {
            let (before, after_open) = rest.split_at(open);
            user.push_str(before);
            let after_open = after_open.get(1..).unwrap_or_default();
            let close = after_open.find('}').ok_or_else(|| {
                InferError::InvalidInput(format!("the template {} has an open brace", self.id))
            })?;
            let (name, after_close) = after_open.split_at(close);
            let value = variables.get(name).ok_or_else(|| {
                InferError::InvalidInput(format!(
                    "the template {} needs the variable {name}",
                    self.id
                ))
            })?;
            user.push_str(value);
            rest = after_close.get(1..).unwrap_or_default();
        }
        user.push_str(rest);
        Ok(vec![Message::system(self.system), Message::user(user)])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn fills_each_variable_once_and_keeps_braces_in_values() {
        let template = PromptTemplate::find("describe_screen").unwrap();
        let variables = BTreeMap::from([
            ("app".to_owned(), "Zed".to_owned()),
            ("title".to_owned(), "{app} main.rs".to_owned()),
        ]);
        let messages = template.render(&variables).unwrap();
        assert_eq!(
            messages[1].content,
            "Application: Zed\nWindow title: {app} main.rs\nDescribe the screenshot."
        );
    }

    #[test]
    fn a_missing_variable_or_template_is_an_error() {
        let template = PromptTemplate::find("describe_screen").unwrap();
        assert!(template.render(&BTreeMap::new()).is_err());
        assert!(PromptTemplate::find("unknown").is_err());
    }
}
