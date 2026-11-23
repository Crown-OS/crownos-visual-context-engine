use atspi::{MatchType, ObjectMatchRule, ObjectRefOwned, State, StateSet};

use super::{NODE_BUDGET, Structure, gather_until};
use crate::a11y::node::NodeFacts;
use crate::a11y::reader::{ReadError, Reader};
use crate::a11y::toolkit::FetchStrategy;

/// One `GetMatches` for every shown node of the window, in canonical order, then role and name
/// for each of them in parallel. A window always shows something, so an empty answer means the
/// toolkit does not really match (Qt 6.12 answers every rule with nothing): `None`, and the
/// caller walks instead.
pub(super) async fn fetch(
    reader: &Reader<'_>,
    frame: &ObjectRefOwned,
) -> Result<Option<Structure>, ReadError> {
    let shown = [State::Showing, State::Visible];
    let rule = ObjectMatchRule::builder()
        .states(shown, MatchType::All)
        .build();
    let objects = reader.matches(frame, rule, NODE_BUDGET).await?;
    if objects.is_empty() {
        return Ok(None);
    }
    let described = gather_until(
        objects
            .iter()
            .enumerate()
            .map(|(index, object)| async move {
                let (role, name) = futures_util::join!(reader.role(object), reader.name(object));
                (index, role, name)
            }),
        reader.deadline(),
    )
    .await;
    let complete = described.len() == objects.len() && objects.len() < NODE_BUDGET;
    let mut nodes: Vec<(usize, NodeFacts)> = described
        .into_iter()
        .filter_map(|(index, role, name)| {
            let object = objects.get(index)?.clone();
            Some((
                index,
                NodeFacts {
                    object,
                    role: role.ok()?,
                    name: name.unwrap_or_default(),
                    states: StateSet::new(State::Showing | State::Visible),
                    has_text_interface: None,
                },
            ))
        })
        .collect();
    nodes.sort_unstable_by_key(|(index, _)| *index);
    Ok(Some(Structure {
        nodes: nodes.into_iter().map(|(_, node)| node).collect(),
        strategy: FetchStrategy::CollectionMatches,
        complete,
    }))
}
