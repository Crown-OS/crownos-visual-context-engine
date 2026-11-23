//! The first request of an app without a grant: the request waits while the
//! user is asked, then runs or fails.

use std::time::Duration;

use context_protocol::context::dispatch_with_peer;
use crownos_config::schema::{AppKey, Scope, ScopeSet};
use crownos_ipc::{Kind, RemoteError, Server};
use tokio::sync::mpsc::UnboundedSender;

use super::parked::Parking;
use super::portal::ask_api_access;
use super::{Grants, Parked, PortalAnswer};
use crate::api::Api;
use crate::pipeline::Outgoing;

/// How long a request waits for the user before it fails.
const QUESTION_DEADLINE: Duration = Duration::from_secs(120);

/// Asks about `key` off the serving loop; the answer comes back as
/// [`Outgoing::Answered`], `None` when nobody answered in time.
pub(crate) fn ask_in_background(key: AppKey, events: &UnboundedSender<Outgoing>) {
    let events = events.clone();
    tokio::spawn(async move {
        let answer = match tokio::time::timeout(QUESTION_DEADLINE, ask_api_access(&key)).await {
            Ok(Ok(answer)) => Some(answer),
            Ok(Err(error)) => {
                tracing::warn!(%error, actor = %key, "nobody could be asked");
                None
            }
            Err(_) => None,
        };
        let _ = events.send(Outgoing::Answered { key, answer });
    });
}

/// Keeps a request of an app the user is not asked about yet, and asks; or
/// refuses it when too many requests wait already.
pub(crate) fn park_or_refuse(
    server: &mut Server,
    parked: &mut Parked,
    key: AppKey,
    peer: crownos_ipc::PeerId,
    message: crownos_ipc::Message,
    events: &UnboundedSender<Outgoing>,
) {
    match parked.park(key.clone(), peer, message) {
        Parking::First => ask_in_background(key, events),
        Parking::Added => {}
        Parking::Full(peer, message) => {
            if message.kind == Kind::Request {
                let _ = server.reply_err(
                    peer,
                    message.req_id,
                    &RemoteError::new(RemoteError::LIMIT, "too many requests wait for the user"),
                );
            }
        }
    }
}

/// Stores the answer and runs, or refuses, the requests that waited for it.
pub(crate) fn answer_parked(
    server: &mut Server,
    grants: &Grants,
    api: &mut Api,
    parked: &mut Parked,
    key: &AppKey,
    answer: Option<PortalAnswer>,
) {
    let scopes = answer.filter(|answer| answer.granted).map(|answer| {
        let metadata = ScopeSet::EMPTY.with(Scope::Metadata);
        if answer.text {
            metadata.with(Scope::Text)
        } else {
            metadata
        }
    });
    if let Some(answer) = answer {
        grants.answer(key, scopes, answer.remember);
    }
    tracing::info!(actor = %key, granted = scopes.is_some(), "first-use answer");
    let refusal = if answer.is_some() {
        "the user did not allow this app to use screen context"
    } else {
        "nobody answered the access question in time"
    };
    for (peer, message) in parked.take(key) {
        if scopes.is_some() {
            if let Err(error) = dispatch_with_peer(api, server, peer, message) {
                tracing::warn!(%error, "a waiting request failed");
            }
        } else if message.kind == Kind::Request {
            let _ = server.reply_err(
                peer,
                message.req_id,
                &RemoteError::new(RemoteError::UNAUTHORIZED, refusal),
            );
        }
    }
}
