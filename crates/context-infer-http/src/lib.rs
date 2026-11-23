//! The HTTP backend: one client for every server with an OpenAI-compatible API (llama-server,
//! Lemonade, FastFlowLM, OVMS, a LAN machine, a cloud). It runs only where the caller has
//! the network and the user's consent; crown-context-inferd does not link it. Images go
//! only to a server on this machine.

mod backend;
mod credentials;
mod embedder;
mod endpoint;
mod generator;
mod response;
mod util;

pub use backend::HttpBackend;
pub use credentials::{CredentialSource, EnvironmentApiKey};
pub use endpoint::{Authentication, Endpoint};
