use std::cell::RefCell;
use std::time::Duration;

use atspi::{CacheItem, ObjectMatchRule, ObjectRefOwned, Role, SortOrder, StateSet};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::Instant;
use zbus::zvariant::{DynamicType, OwnedValue, Type};

use super::calls::AtspiCall;
use super::health::ObjectHealth;
use super::toolkit::Toolkit;

/// A call that used up this much of the budget on its own marks its object hung.
const HUNG_AFTER: Duration = Duration::from_millis(150);
const CACHE_PATH: &str = "/org/a11y/atspi/cache";

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

/// Every read of one application goes through here: the allowlist of its toolkit, the dead and
/// hung objects, and the deadline of the read.
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    connection: &'a zbus::Connection,
    toolkit: Toolkit,
    health: &'a RefCell<ObjectHealth>,
    deadline: Instant,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(
        connection: &'a zbus::Connection,
        toolkit: Toolkit,
        health: &'a RefCell<ObjectHealth>,
        deadline: Instant,
    ) -> Self {
        Self {
            connection,
            toolkit,
            health,
            deadline,
        }
    }

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

    pub(crate) async fn children(
        &self,
        object: &ObjectRefOwned,
    ) -> Result<Vec<ObjectRefOwned>, ReadError> {
        self.invoke(object, AtspiCall::Children, &()).await
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
            self.connection.call_method(
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
