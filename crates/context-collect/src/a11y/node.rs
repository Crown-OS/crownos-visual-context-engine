use std::collections::HashSet;

use atspi::{ObjectRefOwned, Role, State, StateSet};

/// Past this much text in one window the rest is left to OCR.
const MAX_TEXT_BYTES: usize = 64 * 1024;
/// The placeholder a hypertext puts where an embedded child object sits.
const EMBEDDED_OBJECT: char = '\u{fffc}';

/// What the structure pass learned about one accessible.
#[derive(Debug, Clone)]
pub(crate) struct NodeFacts {
    pub(crate) object: ObjectRefOwned,
    pub(crate) role: Role,
    pub(crate) name: String,
    pub(crate) states: StateSet,
    /// `None` when the toolkit did not say which interfaces the node has.
    pub(crate) has_text_interface: Option<bool>,
}

impl NodeFacts {
    pub(crate) fn is_shown(&self) -> bool {
        self.states.contains(State::Showing) && self.states.contains(State::Visible)
    }

    pub(crate) fn is_password_field(&self) -> bool {
        self.role == Role::PasswordText
    }

    pub(crate) fn wants_whole_text(&self) -> bool {
        self.has_text_interface != Some(false) && carries_text(self.role)
    }

    const fn shows_its_name(&self) -> bool {
        matches!(
            self.role,
            Role::Label
                | Role::Heading
                | Role::Link
                | Role::PageTab
                | Role::Static
                | Role::Caption
                | Role::ListItem
                | Role::TableCell
        )
    }
}

const fn carries_text(role: Role) -> bool {
    matches!(
        role,
        Role::Text
            | Role::Entry
            | Role::Paragraph
            | Role::Heading
            | Role::Label
            | Role::Static
            | Role::Link
            | Role::Section
            | Role::Terminal
            | Role::DocumentText
            | Role::DocumentWeb
            | Role::DocumentEmail
            | Role::Caption
            | Role::BlockQuote
            | Role::Comment
            | Role::Article
            | Role::Footnote
            | Role::ListItem
            | Role::TableCell
            | Role::DescriptionTerm
            | Role::DescriptionValue
            | Role::Log
            | Role::Mark
    )
}

/// The lines a window shows, in node order: each node's whole text, or its name when it has no
/// text and is the kind of node whose name is on screen. Repeated lines count once.
pub(crate) fn window_lines<'a>(
    nodes: impl IntoIterator<Item = (&'a NodeFacts, Option<&'a str>)>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    let mut budget = MAX_TEXT_BYTES;
    for (node, text) in nodes {
        let content = match text.filter(|text| !text.trim().is_empty()) {
            Some(text) => text,
            None if node.shows_its_name() => node.name.as_str(),
            None => continue,
        };
        for line in content.split('\n') {
            let line: String = line
                .chars()
                .filter(|character| *character != EMBEDDED_OBJECT)
                .collect();
            let line = line.trim();
            if line.is_empty() || line.len() > budget || !seen.insert(line.to_owned()) {
                continue;
            }
            budget -= line.len();
            lines.push(line.to_owned());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: Role, name: &str) -> NodeFacts {
        NodeFacts {
            object: ObjectRefOwned::from_static_str_unchecked(":1.2", "/a"),
            role,
            name: name.to_owned(),
            states: StateSet::new(State::Showing | State::Visible),
            has_text_interface: None,
        }
    }

    #[test]
    fn text_wins_over_the_name_and_placeholders_go() {
        let paragraph = node(Role::Paragraph, "ignored");
        let label = node(Role::Label, "Subject");
        let button = node(Role::Button, "Send");
        let lines = window_lines([
            (&paragraph, Some("Read the \u{fffc} report\nsecond line")),
            (&label, None),
            (&button, None),
            (&label, None),
        ]);
        assert_eq!(lines, ["Read the  report", "second line", "Subject"]);
    }

    #[test]
    fn only_shown_and_visible_nodes_count_as_shown() {
        let mut hidden = node(Role::Label, "x");
        assert!(hidden.is_shown());
        hidden.states = StateSet::new(State::Visible);
        assert!(!hidden.is_shown());
    }

    #[test]
    fn whole_text_is_asked_only_where_it_can_exist() {
        let mut entry = node(Role::Entry, "");
        assert!(entry.wants_whole_text());
        entry.has_text_interface = Some(false);
        assert!(!entry.wants_whole_text());
        assert!(!node(Role::Button, "OK").wants_whole_text());
        assert!(node(Role::PasswordText, "").is_password_field());
        assert!(!node(Role::PasswordText, "").wants_whole_text());
    }
}
