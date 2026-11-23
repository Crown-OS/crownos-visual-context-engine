use atspi::{ObjectRefOwned, State};

use super::fetch::{Structure, fetch_structure, fetch_text, gather_until};
use super::node::NodeFacts;
use super::reader::{ReadError, Reader};
use super::toolkit::FetchStrategy;

/// What one read of a frame found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameContent {
    Text {
        lines: Vec<String>,
        url: Option<String>,
        strategy: FetchStrategy,
        /// False when the deadline left some text unread.
        complete: bool,
    },
    /// A password field is shown, or a node that did not say its role may be one. No text was
    /// read.
    Sensitive,
    /// The structure was cut short, so a password field may be in the part not seen. No text
    /// was read.
    Incomplete,
}

/// The frame of the application that has the user's attention: the active one, else the first
/// shown one.
pub(crate) async fn active_frame(
    reader: &Reader<'_>,
    root: &ObjectRefOwned,
) -> Result<Option<ObjectRefOwned>, ReadError> {
    let frames = reader.children(root).await?;
    let mut states = gather_until(
        frames.iter().enumerate().map(|(index, frame)| async move {
            (index, reader.state(frame).await.unwrap_or_default())
        }),
        reader.deadline(),
    )
    .await;
    states.sort_unstable_by_key(|(index, _)| *index);
    let pick = states
        .iter()
        .find(|(_, states)| states.contains(State::Active))
        .or_else(|| {
            states
                .iter()
                .find(|(_, states)| states.contains(State::Showing))
        });
    Ok(pick.and_then(|(index, _)| frames.get(*index).cloned()))
}

/// Whether the text of a structure may be read at all.
pub(crate) fn assess(structure: &Structure) -> Option<FrameContent> {
    if structure.unknown_roles || structure.nodes.iter().any(NodeFacts::is_password_field) {
        Some(FrameContent::Sensitive)
    } else if !structure.complete {
        Some(FrameContent::Incomplete)
    } else {
        None
    }
}

/// Structure first; text only when the whole structure was seen and holds no password field.
pub(crate) async fn read_frame(
    reader: &Reader<'_>,
    root: &ObjectRefOwned,
    frame: &ObjectRefOwned,
) -> Result<FrameContent, ReadError> {
    let structure = fetch_structure(reader, root, frame).await?;
    if let Some(verdict) = assess(&structure) {
        return Ok(verdict);
    }
    let text = fetch_text(reader, &structure).await;
    Ok(FrameContent::Text {
        lines: text.lines,
        url: text.url,
        strategy: structure.strategy,
        complete: text.complete,
    })
}

#[cfg(test)]
mod tests {
    use atspi::{Role, StateSet};

    use super::*;

    fn structure(roles: &[Role], complete: bool, unknown_roles: bool) -> Structure {
        Structure {
            nodes: roles
                .iter()
                .map(|role| NodeFacts {
                    object: ObjectRefOwned::from_static_str_unchecked(":1.2", "/a"),
                    role: *role,
                    name: String::new(),
                    states: StateSet::new(State::Showing | State::Visible),
                    has_text_interface: None,
                })
                .collect(),
            strategy: FetchStrategy::BoundedWalk,
            complete,
            unknown_roles,
        }
    }

    #[test]
    fn a_complete_structure_without_secrets_is_read() {
        assert_eq!(
            assess(&structure(&[Role::Frame, Role::Label], true, false)),
            None
        );
    }

    #[test]
    fn a_password_field_or_an_unknown_role_is_sensitive_even_when_cut_short() {
        let password = structure(&[Role::Frame, Role::PasswordText], false, false);
        assert_eq!(assess(&password), Some(FrameContent::Sensitive));
        let unknown = structure(&[Role::Frame], false, true);
        assert_eq!(assess(&unknown), Some(FrameContent::Sensitive));
    }

    #[test]
    fn a_structure_cut_short_gives_no_text() {
        let cut = structure(&[Role::Frame, Role::Label], false, false);
        assert_eq!(assess(&cut), Some(FrameContent::Incomplete));
    }
}
