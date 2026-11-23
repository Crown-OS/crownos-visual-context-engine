use atspi::ObjectRefOwned;

use super::{NODE_BUDGET, Structure, gather_until};
use crate::a11y::node::NodeFacts;
use crate::a11y::reader::{ReadError, Reader};
use crate::a11y::toolkit::FetchStrategy;

/// Where a node sits: the child index at each level from the frame down. Sorting by it gives
/// depth-first order, which is reading order.
type TreePosition = Vec<u16>;

/// Breadth first from the frame, one level at a time with every request of the level in flight
/// together, inside the node budget, the toolkit's depth limit and the deadline.
pub(super) async fn fetch(
    reader: &Reader<'_>,
    frame: &ObjectRefOwned,
) -> Result<Structure, ReadError> {
    let (root, children) = match visit(reader, frame).await {
        Visit::Seen { node, children, .. } => (node, children),
        Visit::RoleUnknown(error) | Visit::Failed(error) => return Err(error),
    };
    let mut nodes: Vec<(TreePosition, NodeFacts)> = vec![(TreePosition::new(), root)];
    let mut frontier = positioned(&[], children);
    let mut complete = true;
    let mut unknown_roles = false;
    for _depth in 1..=reader.toolkit().walk_depth() {
        if frontier.is_empty() {
            break;
        }
        let room = NODE_BUDGET.saturating_sub(nodes.len());
        if frontier.len() > room {
            frontier.truncate(room);
            complete = false;
        }
        let visited = gather_until(
            frontier
                .iter()
                .map(|(position, object)| async move { (position, visit(reader, object).await) }),
            reader.deadline(),
        )
        .await;
        complete &= visited.len() == frontier.len();
        let mut next = Vec::new();
        for (position, outcome) in visited {
            let (node, children, children_complete) = match outcome {
                Visit::Seen {
                    node,
                    children,
                    children_complete,
                } => (node, children, children_complete),
                Visit::RoleUnknown(_) => {
                    unknown_roles = true;
                    complete = false;
                    continue;
                }
                Visit::Failed(_) => {
                    complete = false;
                    continue;
                }
            };
            if !node.is_shown() {
                continue;
            }
            complete &= children_complete;
            next.extend(positioned(position, children));
            nodes.push((position.clone(), node));
        }
        frontier = next;
    }
    complete &= frontier.is_empty();
    nodes.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    Ok(Structure {
        nodes: nodes.into_iter().map(|(_, node)| node).collect(),
        strategy: FetchStrategy::BoundedWalk,
        complete,
        unknown_roles,
    })
}

enum Visit {
    Seen {
        node: NodeFacts,
        children: Vec<ObjectRefOwned>,
        children_complete: bool,
    },
    /// The node may be anything, a password field too.
    RoleUnknown(ReadError),
    /// The role is known, but not whether the node is shown.
    Failed(ReadError),
}

async fn visit(reader: &Reader<'_>, object: &ObjectRefOwned) -> Visit {
    let (role, name, states, children) = futures_util::join!(
        reader.role(object),
        reader.name(object),
        reader.state(object),
        reader.children(object),
    );
    let role = match role {
        Ok(role) => role,
        Err(error) => return Visit::RoleUnknown(error),
    };
    let states = match states {
        Ok(states) => states,
        Err(error) => return Visit::Failed(error),
    };
    let (children, children_complete) = match children {
        Ok(children) => (children.present, !children.skipped_null),
        Err(ReadError::Unsupported) => (Vec::new(), true),
        Err(_) => (Vec::new(), false),
    };
    Visit::Seen {
        node: NodeFacts {
            object: object.clone(),
            role,
            name: name.unwrap_or_default(),
            states,
            has_text_interface: None,
        },
        children,
        children_complete,
    }
}

fn positioned(
    parent: &[u16],
    children: Vec<ObjectRefOwned>,
) -> Vec<(TreePosition, ObjectRefOwned)> {
    children
        .into_iter()
        .enumerate()
        .map(|(index, child)| {
            let mut position = parent.to_vec();
            position.push(u16::try_from(index).unwrap_or(u16::MAX));
            (position, child)
        })
        .collect()
}
