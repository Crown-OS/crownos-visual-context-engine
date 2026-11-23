//! One read of an application's active frame, the way the collector does it but with no window
//! or policy check. For tests only: the `test-support` feature.

use std::cell::RefCell;
use std::time::Duration;

use tokio::time::Instant;

use super::A11yError;
use super::applications::{ApplicationDirectory, root_of};
use super::calls::AtspiCall;
use super::health::ObjectHealth;
use super::read::{FrameContent, active_frame, read_frame};
use super::reader::AccessibilityBusConnection;
use super::toolkit::Toolkit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowReading {
    pub toolkit: Toolkit,
    pub title: String,
    pub content: FrameContent,
    /// Every application call the read made, in order.
    pub calls: Vec<AtspiCall>,
}

/// Reads as `toolkit` when given, whatever the application says it is.
///
/// # Errors
///
/// Fails when the application has no frame or does not answer within `budget`.
pub async fn read_active_window(
    connection: zbus::Connection,
    bus_name: &str,
    budget: Duration,
    toolkit: Option<Toolkit>,
) -> Result<WindowReading, A11yError> {
    let bus = AccessibilityBusConnection::recording(connection);
    let health = RefCell::new(ObjectHealth::default());
    let described = ApplicationDirectory::default()
        .describe(&bus, &health, bus_name)
        .await
        .toolkit;
    let toolkit = toolkit.unwrap_or(described);
    let root = root_of(bus_name).ok_or(A11yError::NoFrame)?;
    let reader = bus.reader(toolkit, &health, Instant::now() + budget);
    let frame = active_frame(&reader, &root)
        .await?
        .ok_or(A11yError::NoFrame)?;
    let title = reader.name(&frame).await.unwrap_or_default();
    let content = read_frame(&reader, &root, &frame).await?;
    Ok(WindowReading {
        toolkit,
        title,
        content,
        calls: bus.recorded_calls(),
    })
}
