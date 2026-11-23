use std::cell::RefCell;
use std::time::Duration;

use atspi::{ObjectRefOwned, State};
use tokio::time::Instant;

use super::A11yError;
use super::applications::{ApplicationDirectory, root_of};
use super::fetch::{fetch_structure, fetch_text, gather_until};
use super::health::ObjectHealth;
use super::node::NodeFacts;
use super::reader::{ReadError, Reader};
use super::toolkit::{FetchStrategy, Toolkit};

/// What one read of a frame found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameContent {
    Text {
        lines: Vec<String>,
        url: Option<String>,
        strategy: FetchStrategy,
        /// False when a budget cut the read short.
        complete: bool,
    },
    /// A password field is shown: no text was read.
    PasswordField,
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

/// Structure first; text only when the structure shows no password field.
pub(crate) async fn read_frame(
    reader: &Reader<'_>,
    root: &ObjectRefOwned,
    frame: &ObjectRefOwned,
) -> Result<FrameContent, ReadError> {
    let structure = fetch_structure(reader, root, frame).await?;
    if structure.nodes.iter().any(NodeFacts::is_password_field) {
        return Ok(FrameContent::PasswordField);
    }
    let text = fetch_text(reader, &structure).await;
    Ok(FrameContent::Text {
        lines: text.lines,
        url: text.url,
        strategy: structure.strategy,
        complete: structure.complete && text.complete,
    })
}

/// One read of the active frame of `bus_name`, the way the collector does it, for tests and
/// tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowReading {
    pub toolkit: Toolkit,
    pub title: String,
    pub content: FrameContent,
}

/// # Errors
///
/// Fails when the application has no frame or does not answer within `budget`.
pub async fn read_active_window(
    connection: &zbus::Connection,
    bus_name: &str,
    budget: Duration,
) -> Result<WindowReading, A11yError> {
    let health = RefCell::new(ObjectHealth::default());
    let info = ApplicationDirectory::default()
        .describe(connection, &health, bus_name)
        .await
        .ok_or(A11yError::NoFrame)?;
    let root = root_of(bus_name).ok_or(A11yError::NoFrame)?;
    let reader = Reader::new(connection, info.toolkit, &health, Instant::now() + budget);
    let frame = active_frame(&reader, &root)
        .await?
        .ok_or(A11yError::NoFrame)?;
    let title = reader.name(&frame).await.unwrap_or_default();
    let content = read_frame(&reader, &root, &frame).await?;
    Ok(WindowReading {
        toolkit: info.toolkit,
        title,
        content,
    })
}
