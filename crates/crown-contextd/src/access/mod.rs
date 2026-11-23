//! Who may ask the Context API what: the grants of `context_access.ron`,
//! checked per request against the caller's `AppKey`.

mod redaction;

use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use crownos_config::schema::{AppKey, ContextAccess, Scope, ScopeSet};
use crownos_ipc::{Caller, RemoteError};

use crate::util::caller_key::caller_key;

pub(crate) use redaction::redact_context;

/// The grants, shared by the request handler and the event sender, and
/// replaced when the file changes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Grants(Arc<RwLock<ContextAccess>>);

/// A caller and what it may do.
#[derive(Debug, Clone)]
pub(crate) struct CallerScopes {
    pub(crate) key: Option<AppKey>,
    pub(crate) scopes: ScopeSet,
}

impl CallerScopes {
    pub(crate) fn require(&self, scope: Scope) -> Result<(), RemoteError> {
        if self.scopes.contains(scope) {
            Ok(())
        } else {
            Err(RemoteError::new(
                RemoteError::UNAUTHORIZED,
                format!("this needs the {scope} scope"),
            ))
        }
    }

    pub(crate) fn require_any(&self) -> Result<(), RemoteError> {
        if self.scopes.is_empty() {
            Err(RemoteError::new(
                RemoteError::UNAUTHORIZED,
                "no grant in context_access.ron",
            ))
        } else {
            Ok(())
        }
    }

    /// The caller as the audit log names it.
    pub(crate) fn actor(&self) -> String {
        self.key
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), ToString::to_string)
    }
}

impl Grants {
    /// Reads the grants strictly: a file that does not parse grants nothing.
    pub(crate) fn load() -> Self {
        let access = crownos_config::load_strict(ContextAccess::SECTION).unwrap_or_else(|error| {
            tracing::warn!(%error, "context_access.ron does not parse; no app gets the API");
            ContextAccess::default()
        });
        Self::from_access(access)
    }

    pub(crate) fn from_access(access: ContextAccess) -> Self {
        Self(Arc::new(RwLock::new(access)))
    }

    pub(crate) fn replace(&self, access: ContextAccess) {
        if let Ok(mut grants) = self.0.write() {
            *grants = access;
        }
    }

    pub(crate) fn of_key(&self, key: Option<AppKey>) -> CallerScopes {
        let scopes = key
            .as_ref()
            .and_then(|key| {
                self.0
                    .read()
                    .ok()
                    .map(|grants| grants.scopes_for(key, SystemTime::now()))
            })
            .unwrap_or_default();
        CallerScopes { key, scopes }
    }

    pub(crate) fn of(&self, caller: Caller<'_>) -> CallerScopes {
        self.of_key(caller_key(caller.credentials))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crownos_config::schema::{Expiry, FirstUsePolicy, Grant};

    use super::*;

    fn grants_for(key: &AppKey, scopes: Vec<Scope>) -> Grants {
        Grants::from_access(ContextAccess {
            host_default: FirstUsePolicy::Deny,
            grants: BTreeMap::from([(
                key.to_string(),
                Grant {
                    scopes,
                    expires: Expiry::Never,
                },
            )]),
            ..ContextAccess::default()
        })
    }

    #[test]
    fn a_caller_gets_exactly_its_grant() {
        let agent = AppKey::Spawned {
            component: "agent".to_owned(),
        };
        let grants = grants_for(&agent, vec![Scope::Metadata, Scope::Text]);
        let caller = grants.of_key(Some(agent));
        assert!(caller.require(Scope::Text).is_ok());
        assert!(caller.require(Scope::MemoryDelete).is_err());
        assert!(caller.require(Scope::Pause).is_err());

        let stranger = grants.of_key(Some(AppKey::Spawned {
            component: "stranger".to_owned(),
        }));
        assert!(stranger.require_any().is_err());
        assert!(grants.of_key(None).require_any().is_err());
    }

    #[test]
    fn a_replaced_file_takes_effect_at_once() {
        let agent = AppKey::Spawned {
            component: "agent".to_owned(),
        };
        let grants = grants_for(&agent, vec![Scope::Metadata]);
        grants.replace(ContextAccess::default());
        assert!(grants.of_key(Some(agent)).require_any().is_err());
    }
}
