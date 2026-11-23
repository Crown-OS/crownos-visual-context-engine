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
pub(crate) const MAX_DEPTH: usize = 12;
/// Requests in flight at once to one application.
const IN_FLIGHT: usize = 32;

/// The shown nodes of one window, in reading order.
#[derive(Debug)]
pub(crate) struct Structure {
    pub(crate) nodes: Vec<NodeFacts>,
    pub(crate) strategy: FetchStrategy,
    /// False when the node budget, the depth limit or the deadline cut the fetch short.
    pub(crate) complete: bool,
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

/// Never called once the structure showed a password field.
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
