use std::collections::HashMap;

use atspi::{CacheItem, Interface, ObjectRefOwned};

use super::{NODE_BUDGET, Structure};
use crate::a11y::node::NodeFacts;
use crate::a11y::reader::{ReadError, Reader};
use crate::a11y::toolkit::FetchStrategy;

/// One `Cache.GetItems` for the whole application, cut down to the shown subtree of the frame.
/// GTK 4 puts an object in its cache only once some client has asked for it, so the cache can
/// miss part of the window. `None` when it misses the frame or a child of a shown node, so the
/// caller walks instead; the walk puts the missing objects in the cache for the next read.
pub(super) async fn fetch(
    reader: &Reader<'_>,
    application: &ObjectRefOwned,
    frame: &ObjectRefOwned,
) -> Result<Option<Structure>, ReadError> {
    let items = reader.cache_items(application).await?;
    Ok(shown_subtree(&items, frame, reader.toolkit().walk_depth()))
}

/// Depth first from `frame` in child index order, which is reading order.
fn shown_subtree(
    items: &[CacheItem],
    frame: &ObjectRefOwned,
    max_depth: usize,
) -> Option<Structure> {
    let mut children: HashMap<&ObjectRefOwned, Vec<&CacheItem>> = HashMap::new();
    let mut root = None;
    for item in items {
        if &item.object == frame {
            root = Some(item);
        }
        children.entry(&item.parent).or_default().push(item);
    }
    for siblings in children.values_mut() {
        siblings.sort_unstable_by_key(|item| item.index);
    }
    let mut nodes = Vec::new();
    let mut complete = true;
    let mut stack = vec![(root?, 0usize)];
    while let Some((item, depth)) = stack.pop() {
        if nodes.len() >= NODE_BUDGET {
            complete = false;
            break;
        }
        let node = NodeFacts {
            object: item.object.clone(),
            role: item.role,
            name: item.short_name.clone(),
            states: item.states,
            has_text_interface: Some(item.ifaces.contains(Interface::Text)),
        };
        if depth > 0 && !node.is_shown() {
            continue;
        }
        nodes.push(node);
        let below = children.get(&item.object).map_or(&[][..], Vec::as_slice);
        if usize::try_from(item.children).unwrap_or(0) > below.len() {
            return None;
        }
        if below.is_empty() {
            continue;
        }
        if depth >= max_depth {
            complete = false;
            continue;
        }
        stack.extend(below.iter().rev().map(|child| (*child, depth + 1)));
    }
    Some(Structure {
        nodes,
        strategy: FetchStrategy::CacheItems,
        complete,
        unknown_roles: false,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use atspi::{InterfaceSet, Role, State, StateSet};

    use super::*;

    fn object(path: &'static str) -> ObjectRefOwned {
        ObjectRefOwned::from_static_str_unchecked(":1.4", path)
    }

    fn item(
        path: &'static str,
        parent: &'static str,
        index: i32,
        children: i32,
        shown: bool,
    ) -> CacheItem {
        let states = if shown {
            StateSet::new(State::Showing | State::Visible)
        } else {
            StateSet::empty()
        };
        CacheItem {
            object: object(path),
            app: object("/org/a11y/atspi/accessible/root"),
            parent: object(parent),
            index,
            children,
            ifaces: InterfaceSet::new(Interface::Accessible | Interface::Text),
            short_name: path.trim_start_matches('/').to_owned(),
            role: Role::Label,
            name: String::new(),
            states,
        }
    }

    #[test]
    fn only_the_shown_subtree_of_the_frame_in_index_order() {
        let items = [
            item("/frame", "/root", 0, 3, true),
            item("/other-frame", "/root", 1, 1, true),
            item("/second", "/frame", 1, 0, true),
            item("/first", "/frame", 0, 1, true),
            item("/hidden", "/frame", 2, 1, false),
            item("/under-first", "/first", 0, 0, true),
            item("/elsewhere", "/other-frame", 0, 0, true),
        ];
        let structure = shown_subtree(&items, &object("/frame"), 12).unwrap();
        let names: Vec<&str> = structure
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, ["frame", "first", "under-first", "second"]);
        assert!(structure.complete);
        assert_eq!(structure.nodes[0].has_text_interface, Some(true));
    }

    #[test]
    fn a_cache_that_misses_part_of_the_window_asks_for_a_walk() {
        let missing_frame = [item("/frame", "/root", 0, 0, true)];
        assert!(shown_subtree(&missing_frame, &object("/gone"), 12).is_none());
        let missing_child = [
            item("/frame", "/root", 0, 2, true),
            item("/first", "/frame", 0, 0, true),
        ];
        assert!(shown_subtree(&missing_child, &object("/frame"), 12).is_none());
    }
}
