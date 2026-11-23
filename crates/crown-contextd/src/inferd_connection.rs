//! The one connection to crown-context-inferd. inferd accepts only `/usr/bin/crown-contextd`,
//! which it reads from `/proc` when it accepts the connection. So contextd connects at startup,
//! before it hides itself and before any frame exists. A reconnect goes through the same
//! exposure hook as the compositor bridge: captures are held while contextd is readable.

use std::path::PathBuf;

use context_collect::{CompositorLink, ExposureRefused, connect_identifiable};
use context_infer::InferError;
use context_infer_client::InferdClient;
use context_protocol::inference::Client;

#[derive(Debug, thiserror::Error)]
enum ConnectError {
    #[error(transparent)]
    Ipc(#[from] crownos_ipc::Error),
    #[error(transparent)]
    Exposure(#[from] ExposureRefused),
}

/// Connects now; call it before `PR_SET_DUMPABLE` goes off. `socket_dir` defaults to
/// `$XDG_RUNTIME_DIR/crownos`.
pub(crate) fn connect(link: CompositorLink, socket_dir: Option<PathBuf>) -> InferdClient {
    InferdClient::with_connector(Box::new(move || {
        let exposure = link.exposure_hook();
        connect_identifiable(exposure.as_ref(), || {
            Ok::<_, ConnectError>(match &socket_dir {
                Some(directory) => Client::connect_in(directory)?,
                None => Client::connect()?,
            })
        })
        .map_err(|error| InferError::Unavailable(format!("crown-context-inferd ({error})").into()))
    }))
}
