//! The few AT-SPI events the collector listens to. Qt and Chromium send only the event types some
//! listener registered, so each registration is as narrow as the registry allows. GTK 4 sends
//! every event once any listener exists.

use atspi::ObjectRefOwned;
use atspi::proxy::registry::RegistryProxy;
use futures_util::{Stream, StreamExt};
use zbus::message::Type as MessageType;
use zbus::zvariant::{Structure, Value};
use zbus::{Message, MessageStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventKind {
    Focused,
    WindowActivated,
    DocumentLoaded,
    NameChanged,
    TextChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccessibilityEvent {
    pub(crate) source: ObjectRefOwned,
    pub(crate) kind: EventKind,
}

impl AccessibilityEvent {
    pub(crate) fn application(&self) -> Option<&str> {
        self.source.name_as_str()
    }
}

struct Subscription {
    registry_event: &'static str,
    interface: &'static str,
    member: &'static str,
    detail: Option<&'static str>,
    kind: EventKind,
}

const SUBSCRIPTIONS: [Subscription; 6] = [
    Subscription {
        registry_event: "focus:",
        interface: "org.a11y.atspi.Event.Focus",
        member: "Focus",
        detail: None,
        kind: EventKind::Focused,
    },
    Subscription {
        registry_event: "object:state-changed:focused",
        interface: "org.a11y.atspi.Event.Object",
        member: "StateChanged",
        detail: Some("focused"),
        kind: EventKind::Focused,
    },
    Subscription {
        registry_event: "window:activate",
        interface: "org.a11y.atspi.Event.Window",
        member: "Activate",
        detail: None,
        kind: EventKind::WindowActivated,
    },
    Subscription {
        registry_event: "document:load-complete",
        interface: "org.a11y.atspi.Event.Document",
        member: "LoadComplete",
        detail: None,
        kind: EventKind::DocumentLoaded,
    },
    Subscription {
        registry_event: "object:property-change:accessible-name",
        interface: "org.a11y.atspi.Event.Object",
        member: "PropertyChange",
        detail: Some("accessible-name"),
        kind: EventKind::NameChanged,
    },
    Subscription {
        registry_event: "object:text-changed",
        interface: "org.a11y.atspi.Event.Object",
        member: "TextChanged",
        detail: None,
        kind: EventKind::TextChanged,
    },
];

/// Per subscription, the events that may wait before the collector takes them.
const QUEUED_PER_SUBSCRIPTION: usize = 256;

impl Subscription {
    fn match_rule(&self) -> String {
        let detail = self
            .detail
            .map(|detail| format!(",arg0='{detail}'"))
            .unwrap_or_default();
        format!(
            "type='signal',interface='{}',member='{}'{detail}",
            self.interface, self.member
        )
    }
}

/// Adds the match rules on the bus and tells the registry which events to forward.
pub(crate) async fn subscribe(
    connection: &zbus::Connection,
) -> Result<impl Stream<Item = AccessibilityEvent> + Send + 'static, zbus::Error> {
    let registry = RegistryProxy::new(connection).await?;
    let mut streams = Vec::with_capacity(SUBSCRIPTIONS.len());
    for subscription in &SUBSCRIPTIONS {
        streams.push(
            MessageStream::for_match_rule(
                subscription.match_rule().as_str(),
                connection,
                Some(QUEUED_PER_SUBSCRIPTION),
            )
            .await?,
        );
        registry.register_event(subscription.registry_event).await?;
    }
    Ok(futures_util::stream::select_all(streams)
        .filter_map(|message| std::future::ready(message.ok().as_ref().and_then(parse))))
}

/// Reads the header, and the body only for the events filtered by detail: the detail string
/// and, for a state change, whether the state was gained. A text change, which comes with every
/// keystroke, is never deserialized, so the changed text itself is never read.
pub(crate) fn parse(message: &Message) -> Option<AccessibilityEvent> {
    if message.message_type() != MessageType::Signal {
        return None;
    }
    let header = message.header();
    let interface = header.interface()?.as_str();
    let member = header.member()?.as_str();
    let subscription = SUBSCRIPTIONS.iter().find(|subscription| {
        subscription.interface == interface && subscription.member == member
    })?;
    if let Some(wanted) = subscription.detail {
        let body = message.body();
        let fields: Structure<'_> = body.deserialize().ok()?;
        let mut fields = fields.fields().iter();
        let detail_matches =
            matches!(fields.next(), Some(Value::Str(detail)) if detail.as_str() == wanted);
        let gained = matches!(fields.next(), Some(Value::I32(1)));
        if !detail_matches || (member == "StateChanged" && !gained) {
            return None;
        }
    }
    let source =
        atspi::ObjectRef::new_owned(header.sender()?.to_owned(), header.path()?.to_owned());
    Some(AccessibilityEvent {
        source,
        kind: subscription.kind,
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
    use std::collections::HashMap;

    use zbus::zvariant::OwnedValue;

    use super::*;

    fn signal(interface: &str, member: &str, detail: &str, gained: i32) -> Message {
        let properties: HashMap<String, OwnedValue> = HashMap::new();
        Message::signal("/org/a11y/atspi/accessible/42", interface, member)
            .unwrap()
            .sender(":1.77")
            .unwrap()
            .build(&(detail, gained, 0i32, Value::from(0u32), properties))
            .unwrap()
    }

    #[test]
    fn a_gained_focus_names_its_application_and_object() {
        let event = parse(&signal(
            "org.a11y.atspi.Event.Object",
            "StateChanged",
            "focused",
            1,
        ))
        .unwrap();
        assert_eq!(event.kind, EventKind::Focused);
        assert_eq!(event.application(), Some(":1.77"));
        assert_eq!(event.source.path_as_str(), "/org/a11y/atspi/accessible/42");
    }

    #[test]
    fn lost_focus_and_other_states_are_ignored() {
        let object = "org.a11y.atspi.Event.Object";
        assert!(parse(&signal(object, "StateChanged", "focused", 0)).is_none());
        assert!(parse(&signal(object, "StateChanged", "checked", 1)).is_none());
        assert!(
            parse(&signal(
                object,
                "PropertyChange",
                "accessible-description",
                0
            ))
            .is_none()
        );
    }

    #[test]
    fn every_subscription_parses_back_to_its_kind() {
        for subscription in &SUBSCRIPTIONS {
            let message = signal(
                subscription.interface,
                subscription.member,
                subscription.detail.unwrap_or("insert"),
                1,
            );
            assert_eq!(parse(&message).unwrap().kind, subscription.kind);
        }
    }

    #[test]
    fn match_rules_narrow_by_detail() {
        assert_eq!(
            SUBSCRIPTIONS[1].match_rule(),
            "type='signal',interface='org.a11y.atspi.Event.Object',member='StateChanged',\
             arg0='focused'"
        );
        assert!(!SUBSCRIPTIONS[5].match_rule().contains("arg0"));
    }
}
