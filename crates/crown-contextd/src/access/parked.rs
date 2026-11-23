//! Requests from callers whose app is being asked about, kept until the
//! user answers or the question times out.

use std::collections::HashMap;

use crownos_config::schema::AppKey;
use crownos_ipc::{Message, PeerId};

/// How many apps may wait for an answer at once.
const MAX_APPS: usize = 32;
/// How many requests of one app may wait for its answer.
const MAX_REQUESTS_PER_APP: usize = 16;

/// Requests by the peer they came from; the peer type is generic so tests
/// need no live connection.
#[derive(Debug)]
pub(crate) struct Parked<Peer = PeerId> {
    waiting: HashMap<AppKey, Vec<(Peer, Message)>>,
}

impl<Peer> Default for Parked<Peer> {
    fn default() -> Self {
        Self {
            waiting: HashMap::new(),
        }
    }
}

/// What became of a request that waits for the user.
#[derive(Debug)]
pub(crate) enum Parking<Peer> {
    /// The first request of the app: it needs a question.
    First,
    /// The app is asked about already.
    Added,
    /// Too many requests wait; this one is handed back.
    Full(Peer, Message),
}

impl<Peer> Parked<Peer> {
    /// Keeps `message` until `key` is answered, up to the limits.
    pub(crate) fn park(&mut self, key: AppKey, peer: Peer, message: Message) -> Parking<Peer> {
        if !self.waiting.contains_key(&key) && self.waiting.len() >= MAX_APPS {
            return Parking::Full(peer, message);
        }
        let entry = self.waiting.entry(key).or_default();
        if entry.len() >= MAX_REQUESTS_PER_APP {
            return Parking::Full(peer, message);
        }
        entry.push((peer, message));
        if entry.len() == 1 {
            Parking::First
        } else {
            Parking::Added
        }
    }

    pub(crate) fn take(&mut self, key: &AppKey) -> Vec<(Peer, Message)> {
        self.waiting.remove(key).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use crownos_ipc::Kind;

    use super::*;

    fn message() -> Message {
        Message {
            kind: Kind::Request,
            selector: 0,
            req_id: 0,
            payload: Vec::new(),
            fds: Vec::new(),
        }
    }

    #[test]
    fn the_first_request_asks_and_too_many_are_handed_back() {
        let peer = 7_u32;
        let mut parked = Parked::<u32>::default();
        let key = AppKey::Spawned {
            component: "agent".to_owned(),
        };
        assert!(matches!(
            parked.park(key.clone(), peer, message()),
            Parking::First
        ));
        for _ in 1..MAX_REQUESTS_PER_APP {
            assert!(matches!(
                parked.park(key.clone(), peer, message()),
                Parking::Added
            ));
        }
        assert!(matches!(
            parked.park(key.clone(), peer, message()),
            Parking::Full(..)
        ));
        assert_eq!(parked.take(&key).len(), MAX_REQUESTS_PER_APP);
    }
}
