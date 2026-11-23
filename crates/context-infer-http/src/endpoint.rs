use std::path::PathBuf;

use serde::Deserialize;

/// One OpenAI-compatible server, as a config file names it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Endpoint {
    /// For example `http://127.0.0.1:8080` or `https://spark.lan:8443`; `/v1/...` is added.
    pub base_url: String,
    #[serde(default)]
    pub authentication: Authentication,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Authentication {
    #[default]
    None,
    /// A bearer key read from this environment variable at each request, never stored.
    ApiKeyFromEnvironment { variable: String },
    /// Client certificate and key (PEM) for a LAN machine paired with this one, and the CA
    /// that signed the server.
    MutualTls {
        certificate: PathBuf,
        private_key: PathBuf,
        authority: PathBuf,
    },
}
