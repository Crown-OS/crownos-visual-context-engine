//! Who may ask the Context API what: the grants of `context_access.ron`,
//! checked per request against the caller's `AppKey`.

mod first_use;
mod parked;
mod portal;
mod redaction;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crownos_config::schema::{AppKey, ContextAccess, ContextDecision, Expiry, Scope, ScopeSet};
use crownos_ipc::{Caller, PeerId, RemoteError};

pub(crate) use first_use::{answer_parked, park_or_refuse};
pub(crate) use parked::Parked;
pub(crate) use portal::PortalAnswer;
pub(crate) use redaction::redact_context;

/// The grants, shared by the request handler and the event sender, and
/// replaced when the file changes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Grants {
    access: Arc<RwLock<ContextAccess>>,
    /// Answers the user gave without remembering them; they last until the
    /// grants change or contextd stops.
    answered: Arc<RwLock<HashMap<AppKey, ScopeSet>>>,
    /// Apps the user refused without remembering, and when they may ask again.
    cooldown: Arc<RwLock<HashMap<AppKey, Instant>>>,
    /// The key of each peer, worked out once from the program it ran when it
    /// connected.
    keys: Arc<RwLock<HashMap<PeerId, Option<AppKey>>>>,
}

/// After the user refuses without remembering, the app is not asked again
/// for this long, so it cannot ask over and over by calling again.
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(10 * 60);

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
        Self {
            access: Arc::new(RwLock::new(access)),
            answered: Arc::default(),
            cooldown: Arc::default(),
            keys: Arc::default(),
        }
    }

    /// Takes the new file. Answers given without remembering end with it,
    /// so a caller that only had one is dropped as well.
    pub(crate) fn replace(&self, access: ContextAccess) {
        if let Ok(mut grants) = self.access.write() {
            *grants = access;
        }
        if let Ok(mut answered) = self.answered.write() {
            answered.clear();
        }
    }

    /// The decision for a caller; one that cannot be named is refused.
    pub(crate) fn decide(&self, key: Option<&AppKey>) -> ContextDecision {
        let Some(key) = key else {
            return ContextDecision::Denied;
        };
        let decision = self
            .access
            .read()
            .map_or(ContextDecision::Denied, |access| {
                access.decide(key, SystemTime::now())
            });
        if decision != ContextDecision::Ask {
            return decision;
        }
        if let Some(scopes) = self
            .answered
            .read()
            .ok()
            .and_then(|answered| answered.get(key).copied())
        {
            return ContextDecision::Granted(scopes);
        }
        let cooling_down = self.cooldown.read().is_ok_and(|cooldown| {
            cooldown
                .get(key)
                .is_some_and(|until| *until > Instant::now())
        });
        if cooling_down {
            ContextDecision::Denied
        } else {
            ContextDecision::Ask
        }
    }

    /// Applies the user's answer: a remembered one is written to
    /// `context_access.ron` with a strict load, so a broken file is never
    /// replaced; any other one lasts while contextd runs.
    pub(crate) fn answer(&self, key: &AppKey, scopes: Option<ScopeSet>, remember: bool) {
        if !remember {
            match scopes {
                Some(scopes) => {
                    if let Ok(mut answered) = self.answered.write() {
                        answered.insert(key.clone(), scopes);
                    }
                }
                None => {
                    if let Ok(mut cooldown) = self.cooldown.write() {
                        cooldown.retain(|_, until| *until > Instant::now());
                        cooldown.insert(key.clone(), Instant::now() + ASK_AGAIN_AFTER);
                    }
                }
            }
            return;
        }
        let mut access: ContextAccess = match crownos_config::load_strict(ContextAccess::SECTION) {
            Ok(access) => access,
            Err(error) => {
                tracing::warn!(%error, "cannot remember the answer");
                return;
            }
        };
        match scopes {
            Some(scopes) => access.grant(key, scopes, Expiry::Never),
            None => access.deny(key),
        }
        if let Err(error) = crownos_config::save(ContextAccess::SECTION, &access) {
            tracing::warn!(%error, "cannot remember the answer");
            return;
        }
        self.replace(access);
    }

    pub(crate) fn of_key(&self, key: Option<AppKey>) -> CallerScopes {
        let scopes = match self.decide(key.as_ref()) {
            ContextDecision::Granted(scopes) => scopes,
            ContextDecision::Ask | ContextDecision::Denied => ScopeSet::EMPTY,
        };
        CallerScopes { key, scopes }
    }

    /// The scopes of a caller, named by the program it ran when it
    /// connected. A caller whose program could not be read then has none.
    pub(crate) fn of(&self, caller: Caller<'_>) -> CallerScopes {
        self.of_key(self.key_of(caller))
    }

    /// The key of a caller, worked out on its first request and kept.
    pub(crate) fn key_of(&self, caller: Caller<'_>) -> Option<AppKey> {
        if let Some(key) = self
            .keys
            .read()
            .ok()
            .and_then(|keys| keys.get(&caller.peer).cloned())
        {
            return key;
        }
        let key = caller_key(caller);
        if let Ok(mut keys) = self.keys.write() {
            keys.insert(caller.peer, key.clone());
        }
        key
    }

    /// Forgets the keys of peers that are gone.
    pub(crate) fn keep_keys_of(&self, peers: &[PeerId]) {
        if let Ok(mut keys) = self.keys.write() {
            keys.retain(|peer, _| peers.contains(peer));
        }
    }
}

/// The key of a caller, from the program crownos-ipc read at accept.
fn caller_key(caller: Caller<'_>) -> Option<AppKey> {
    let program = caller.credentials.program.as_ref()?;
    AppKey::of_program(
        &program.executable,
        &program.arguments,
        program.working_directory.as_deref(),
    )
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

    #[test]
    fn an_answer_without_remembering_lasts_while_contextd_runs() {
        let agent = AppKey::Spawned {
            component: "agent".to_owned(),
        };
        let grants = Grants::from_access(ContextAccess::default());
        assert_eq!(grants.decide(Some(&agent)), ContextDecision::Ask);
        let metadata = ScopeSet::EMPTY.with(Scope::Metadata);
        grants.answer(&agent, Some(metadata), false);
        assert_eq!(
            grants.decide(Some(&agent)),
            ContextDecision::Granted(metadata)
        );
        assert_eq!(grants.decide(None), ContextDecision::Denied);
    }

    #[test]
    fn a_new_file_ends_answers_that_were_not_remembered() {
        let agent = AppKey::Spawned {
            component: "agent".to_owned(),
        };
        let grants = Grants::from_access(ContextAccess::default());
        grants.answer(&agent, Some(ScopeSet::EMPTY.with(Scope::Metadata)), false);
        grants.replace(ContextAccess::default());
        assert_eq!(grants.decide(Some(&agent)), ContextDecision::Ask);
    }

    #[test]
    fn a_refusal_that_is_not_remembered_stops_new_questions_for_a_while() {
        let agent = AppKey::Spawned {
            component: "agent".to_owned(),
        };
        let grants = Grants::from_access(ContextAccess::default());
        grants.answer(&agent, None, false);
        assert_eq!(grants.decide(Some(&agent)), ContextDecision::Denied);
    }
}
