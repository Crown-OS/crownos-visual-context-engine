use std::cell::RefCell;
use std::time::Duration;

use atspi::{CacheItem, ObjectMatchRule, ObjectRefOwned, Role, SortOrder, StateSet};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::Instant;
use zbus::names::BusName;
use zbus::zvariant::{DynamicType, OwnedValue, Type};

use super::calls::AtspiCall;
use super::events::AccessibilityEvent;
use super::health::ObjectHealth;
use super::toolkit::Toolkit;

/// A call that used up this much of the budget on its own marks its object hung.
const HUNG_AFTER: Duration = Duration::from_millis(150);
const CACHE_PATH: &str = "/org/a11y/atspi/cache";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const NULL_PATH: &str = "/org/a11y/atspi/null";

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0:?} is not allowed for this toolkit")]
    NotPermitted(AtspiCall),
    #[error("the object is known to be gone or hung")]
    Skipped,
    #[error("the object is gone")]
    Gone,
    #[error("the object does not implement the call")]
    Unsupported,
    #[error("the read budget ran out")]
    OutOfTime,
    #[error("the a11y bus failed: {0}")]
    Bus(#[from] zbus::Error),
    #[error("the reply did not parse: {0}")]
    Reply(#[from] zbus::zvariant::Error),
}

/// The children an accessible lists. Chromium lists a null object where it has not built a
/// subtree yet: that child is left out, and the list is known to be partial.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Children {
    pub(crate) present: Vec<ObjectRefOwned>,
    pub(crate) skipped_null: bool,
}

impl Children {
    fn from_listed(listed: Vec<ObjectRefOwned>) -> Self {
        let before = listed.len();
        let present: Vec<ObjectRefOwned> = listed
            .into_iter()
            .filter(|child| !child.is_null() && child.path_as_str() != NULL_PATH)
            .collect();
        Self {
            skipped_null: present.len() < before,
            present,
        }
    }
}

/// The a11y bus. The connection never leaves this module: applications are called only through
/// [`Reader::invoke`], and the bus itself only for pids, the registry and event subscriptions.
#[derive(Debug)]
pub(crate) struct AccessibilityBusConnection {
    connection: zbus::Connection,
    recorded_calls: Option<RefCell<Vec<AtspiCall>>>,
}

impl AccessibilityBusConnection {
    pub(crate) const fn new(connection: zbus::Connection) -> Self {
        Self {
            connection,
            recorded_calls: None,
        }
    }

    /// Keeps a list of every application call, for tests of the allowlist.
    #[cfg(feature = "test-support")]
    pub(crate) const fn recording(connection: zbus::Connection) -> Self {
        Self {
            connection,
            recorded_calls: Some(RefCell::new(Vec::new())),
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn recorded_calls(&self) -> Vec<AtspiCall> {
        self.recorded_calls
            .as_ref()
            .map(|calls| calls.borrow().clone())
            .unwrap_or_default()
    }

    pub(crate) const fn reader<'a>(
        &'a self,
        toolkit: Toolkit,
        health: &'a RefCell<ObjectHealth>,
        deadline: Instant,
    ) -> Reader<'a> {
        Reader {
            bus: self,
            toolkit,
            health,
            deadline,
        }
    }

    pub(crate) async fn subscribe(
        &self,
    ) -> Result<impl futures_util::Stream<Item = AccessibilityEvent> + Send + 'static, zbus::Error>
    {
        super::events::subscribe(&self.connection).await
    }

    /// The pid behind a unique name, from the bus daemon.
    pub(crate) async fn process_of(&self, bus_name: &str) -> Option<i32> {
        let dbus = zbus::fdo::DBusProxy::new(&self.connection).await.ok()?;
        let name = BusName::try_from(bus_name).ok()?;
        let pid = dbus.get_connection_unix_process_id(name).await.ok()?;
        i32::try_from(pid).ok()
    }

    /// The unique name of the registry, whose root lists the applications.
    pub(crate) async fn registry_name(&self) -> Option<String> {
        let dbus = zbus::fdo::DBusProxy::new(&self.connection).await.ok()?;
        let registry = BusName::try_from(REGISTRY).ok()?;
        let owner = dbus.get_name_owner(registry).await.ok()?;
        Some(owner.to_string())
    }
}

/// Every read of one application goes through here: the allowlist of its toolkit, the dead and
/// hung objects, and the deadline of the read.
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    bus: &'a AccessibilityBusConnection,
    toolkit: Toolkit,
    health: &'a RefCell<ObjectHealth>,
    deadline: Instant,
}

impl Reader<'_> {
    pub(crate) const fn toolkit(&self) -> Toolkit {
        self.toolkit
    }

    pub(crate) const fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) async fn role(&self, object: &ObjectRefOwned) -> Result<Role, ReadError> {
        let role: u32 = self.invoke(object, AtspiCall::Role, &()).await?;
        Ok(Role::try_from(role).unwrap_or(Role::Invalid))
    }

    pub(crate) async fn name(&self, object: &ObjectRefOwned) -> Result<String, ReadError> {
        let value: OwnedValue = self
            .invoke(
                object,
                AtspiCall::Name,
                &("org.a11y.atspi.Accessible", "Name"),
            )
            .await?;
        Ok(String::try_from(value)?)
    }

    pub(crate) async fn state(&self, object: &ObjectRefOwned) -> Result<StateSet, ReadError> {
        self.invoke(object, AtspiCall::State, &()).await
    }

    pub(crate) async fn children(&self, object: &ObjectRefOwned) -> Result<Children, ReadError> {
        let listed: Vec<ObjectRefOwned> = self.invoke(object, AtspiCall::Children, &()).await?;
        Ok(Children::from_listed(listed))
    }

    pub(crate) async fn whole_text(&self, object: &ObjectRefOwned) -> Result<String, ReadError> {
        self.invoke(object, AtspiCall::WholeText, &(0i32, -1i32))
            .await
    }

    pub(crate) async fn document_url(&self, object: &ObjectRefOwned) -> Result<String, ReadError> {
        self.invoke(object, AtspiCall::DocumentUrl, &"DocURL").await
    }

    /// The `ToolkitName` and `Version` of the application whose root is `root`.
    pub(crate) async fn toolkit_of(
        &self,
        root: &ObjectRefOwned,
    ) -> Result<(String, String), ReadError> {
        const APPLICATION: &str = "org.a11y.atspi.Application";
        let (name, version) = futures_util::join!(
            self.invoke::<_, OwnedValue>(root, AtspiCall::Toolkit, &(APPLICATION, "ToolkitName")),
            self.invoke::<_, OwnedValue>(root, AtspiCall::Toolkit, &(APPLICATION, "Version")),
        );
        Ok((
            String::try_from(name?)?,
            String::try_from(version?).unwrap_or_default(),
        ))
    }

    pub(crate) async fn matches(
        &self,
        container: &ObjectRefOwned,
        rule: ObjectMatchRule,
        limit: usize,
    ) -> Result<Vec<ObjectRefOwned>, ReadError> {
        let count = i32::try_from(limit).unwrap_or(i32::MAX);
        let body = (rule, SortOrder::Canonical, count, false);
        self.invoke(container, AtspiCall::CollectionMatches, &body)
            .await
    }

    pub(crate) async fn cache_items(
        &self,
        application: &ObjectRefOwned,
    ) -> Result<Vec<CacheItem>, ReadError> {
        let bus_name = application.name().ok_or(ReadError::Gone)?.clone();
        let cache = atspi::ObjectRef::new_owned(
            bus_name,
            zbus::zvariant::ObjectPath::from_static_str_unchecked(CACHE_PATH),
        );
        self.invoke(&cache, AtspiCall::CacheItems, &()).await
    }

    async fn invoke<B, R>(
        &self,
        object: &ObjectRefOwned,
        call: AtspiCall,
        body: &B,
    ) -> Result<R, ReadError>
    where
        B: Serialize + DynamicType,
        R: DeserializeOwned + Type,
    {
        if !call.permitted(self.toolkit) {
            return Err(ReadError::NotPermitted(call));
        }
        if let Some(calls) = &self.bus.recorded_calls {
            calls.borrow_mut().push(call);
        }
        let issued = Instant::now();
        if issued >= self.deadline {
            return Err(ReadError::OutOfTime);
        }
        if !self.health.borrow().is_usable(object, issued.into_std()) {
            return Err(ReadError::Skipped);
        }
        let destination = object.name_as_str().ok_or(ReadError::Gone)?;
        let (interface, member) = call.wire();
        let reply = tokio::time::timeout_at(
            self.deadline,
            self.bus.connection.call_method(
                Some(destination),
                object.path_as_str(),
                Some(interface),
                member,
                body,
            ),
        )
        .await;
        match reply {
            Ok(Ok(message)) => Ok(message.body().deserialize::<R>()?),
            Ok(Err(error)) => Err(self.classify(object, error)),
            Err(_) => {
                if issued.elapsed() >= HUNG_AFTER {
                    self.health
                        .borrow_mut()
                        .mark_hung(object, issued.into_std());
                }
                Err(ReadError::OutOfTime)
            }
        }
    }

    fn classify(&self, object: &ObjectRefOwned, error: zbus::Error) -> ReadError {
        let zbus::Error::MethodError(name, _, _) = &error else {
            return ReadError::Bus(error);
        };
        let name = name.as_str();
        if name.ends_with("UnknownMethod") || name.ends_with("UnknownInterface") {
            ReadError::Unsupported
        } else if name.ends_with("UnknownObject")
            || name.ends_with("ServiceUnknown")
            || name.ends_with("NameHasNoOwner")
        {
            self.health.borrow_mut().mark_dead(object);
            ReadError::Gone
        } else if name.ends_with("NoReply") {
            self.health
                .borrow_mut()
                .mark_hung(object, std::time::Instant::now());
            ReadError::OutOfTime
        } else {
            ReadError::Bus(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_null_child_is_left_out_and_marks_the_list_partial() {
        let real =
            ObjectRefOwned::from_static_str_unchecked(":1.4", "/org/a11y/atspi/accessible/7");
        let null = ObjectRefOwned::from_static_str_unchecked(":1.4", NULL_PATH);
        let children = Children::from_listed(vec![real.clone(), null]);
        assert_eq!(children.present, std::slice::from_ref(&real));
        assert!(children.skipped_null);
        assert!(!Children::from_listed(vec![real]).skipped_null);
    }
}
