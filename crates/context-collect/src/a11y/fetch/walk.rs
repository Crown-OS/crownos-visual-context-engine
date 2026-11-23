use atspi::ObjectRefOwned;

use super::{MAX_DEPTH, NODE_BUDGET, Structure, gather_until};
use crate::a11y::node::NodeFacts;
use crate::a11y::reader::{ReadError, Reader};
use crate::a11y::toolkit::FetchStrategy;

/// Where a node sits: the child index at each level from the frame down. Sorting by it gives
/// depth-first order, which is reading order.
type TreePosition = Vec<u16>;

/// Breadth first from the frame, one level at a time with every request of the level in flight
/// together, inside the node budget, the depth limit and the deadline.
pub(super) async fn fetch(
    reader: &Reader<'_>,
    frame: &ObjectRefOwned,
) -> Result<Structure, ReadError> {
    let (root, children) = visit(reader, frame).await?;
    let mut nodes: Vec<(TreePosition, NodeFacts)> = vec![(TreePosition::new(), root)];
    let mut frontier = positioned(&[], children);
    let mut complete = true;
    for _depth in 1..=MAX_DEPTH {
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
            let Ok((node, children)) = outcome else {
                continue;
            };
            if !node.is_shown() {
                continue;
            }
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
    })
}

async fn visit(
    reader: &Reader<'_>,
    object: &ObjectRefOwned,
) -> Result<(NodeFacts, Vec<ObjectRefOwned>), ReadError> {
    let (role, name, states, children) = futures_util::join!(
        reader.role(object),
        reader.name(object),
        reader.state(object),
        reader.children(object),
    );
    let node = NodeFacts {
        object: object.clone(),
        role: role?,
        name: name.unwrap_or_default(),
        states: states?,
        has_text_interface: None,
    };
    Ok((node, children.unwrap_or_default()))
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
