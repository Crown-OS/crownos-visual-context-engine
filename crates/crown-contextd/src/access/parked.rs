//! Requests from callers whose app is being asked about, kept until the
//! user answers or the question times out.

use std::collections::HashMap;

use crownos_config::schema::AppKey;
use crownos_ipc::{Message, PeerId};

#[derive(Debug, Default)]
pub(crate) struct Parked {
    waiting: HashMap<AppKey, Vec<(PeerId, Message)>>,
}

impl Parked {
    /// Keeps `message` until `key` is answered. Returns whether this is the
    /// first request for `key`, which then needs a question.
    pub(crate) fn park(&mut self, key: AppKey, peer: PeerId, message: Message) -> bool {
        let entry = self.waiting.entry(key).or_default();
        entry.push((peer, message));
        entry.len() == 1
    }

    pub(crate) fn take(&mut self, key: &AppKey) -> Vec<(PeerId, Message)> {
        self.waiting.remove(key).unwrap_or_default()
    }
}
