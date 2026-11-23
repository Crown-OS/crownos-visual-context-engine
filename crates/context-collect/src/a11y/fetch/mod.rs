//! The two passes of a read: the structure of the focused window by the cheapest strategy its
//! toolkit allows, then the text of the shown nodes that carry text.

mod cache;
mod collection;
mod walk;

use std::future::Future;

use atspi::{ObjectRefOwned, Role};
use futures_util::StreamExt;
use tokio::time::Instant;

use super::node::NodeFacts;
use super::reader::{ReadError, Reader};
use super::toolkit::FetchStrategy;

pub(crate) const NODE_BUDGET: usize = 2_000;
/// Requests in flight at once to one application.
const IN_FLIGHT: usize = 32;

/// The shown nodes of one window, in reading order.
#[derive(Debug)]
pub(crate) struct Structure {
    pub(crate) nodes: Vec<NodeFacts>,
    pub(crate) strategy: FetchStrategy,
    /// False when the node budget, the depth limit, the deadline or a failed call cut the fetch
    /// short.
    pub(crate) complete: bool,
    /// Some node did not say its role, so it may be a password field.
    pub(crate) unknown_roles: bool,
}

#[derive(Debug)]
pub(crate) struct Text {
    pub(crate) lines: Vec<String>,
    pub(crate) url: Option<String>,
    pub(crate) complete: bool,
}

pub(crate) async fn fetch_structure(
    reader: &Reader<'_>,
    application: &ObjectRefOwned,
    frame: &ObjectRefOwned,
) -> Result<Structure, ReadError> {
    let bulk = match reader.toolkit().strategy() {
        FetchStrategy::CollectionMatches => collection::fetch(reader, frame).await,
        FetchStrategy::CacheItems => cache::fetch(reader, application, frame).await,
        FetchStrategy::BoundedWalk => return walk::fetch(reader, frame).await,
    };
    match bulk {
        Ok(Some(structure)) => Ok(structure),
        Ok(None) | Err(ReadError::Unsupported) => walk::fetch(reader, frame).await,
        Err(error) => Err(error),
    }
}

/// Called only for a complete structure with no password field in it.
pub(crate) async fn fetch_text(reader: &Reader<'_>, structure: &Structure) -> Text {
    let wanted: Vec<(usize, &NodeFacts)> = structure
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.wants_whole_text())
        .collect();
    let document = structure
        .nodes
        .iter()
        .find(|node| node.role == Role::DocumentWeb);
    let texts = async {
        gather_until(
            wanted.iter().map(|(index, node)| async move {
                (*index, reader.whole_text(&node.object).await.ok())
            }),
            reader.deadline(),
        )
        .await
    };
    let url = async {
        match document {
            Some(document) => reader
                .document_url(&document.object)
                .await
                .ok()
                .filter(|url| !url.is_empty()),
            None => None,
        }
    };
    let (texts, url) = futures_util::join!(texts, url);
    let complete = texts.len() == wanted.len();
    let mut by_node: Vec<Option<String>> = vec![None; structure.nodes.len()];
    for (index, text) in texts {
        if let Some(slot) = by_node.get_mut(index) {
            *slot = text;
        }
    }
    let lines = super::node::window_lines(
        structure
            .nodes
            .iter()
            .zip(by_node.iter().map(Option::as_deref)),
    );
    Text {
        lines,
        url,
        complete,
    }
}

/// Runs up to [`IN_FLIGHT`] of `requests` at a time and keeps what answered before `deadline`.
pub(crate) async fn gather_until<T>(
    requests: impl Iterator<Item = impl Future<Output = T>>,
    deadline: Instant,
) -> Vec<T> {
    let mut answers = futures_util::stream::iter(requests).buffer_unordered(IN_FLIGHT);
    let mut gathered = Vec::new();
    while let Ok(Some(answer)) = tokio::time::timeout_at(deadline, answers.next()).await {
        gathered.push(answer);
    }
    gathered
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn answers_after_the_deadline_are_left_out_and_the_rest_kept() {
        let deadline = Instant::now() + Duration::from_millis(150);
        let delays = [10u64, 600, 20, 700, 30];
        let mut gathered = gather_until(
            delays.iter().map(|delay| async move {
                tokio::time::sleep(Duration::from_millis(*delay)).await;
                *delay
            }),
            deadline,
        )
        .await;
        gathered.sort_unstable();
        assert_eq!(gathered, [10, 20, 30]);
        assert!(Instant::now() < deadline + Duration::from_millis(100));
    }
}
