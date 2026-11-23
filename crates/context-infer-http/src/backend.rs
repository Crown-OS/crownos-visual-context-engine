use std::path::Path;
use std::sync::Arc;

use context_infer::registry::Artifact;
use context_infer::{Device, Embed, Generate, InferError, LoadSpec, Runtime, RuntimeBackend};
use ureq::Agent;
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};

use crate::credentials::{CredentialSource, EnvironmentApiKey, NoCredentials};
use crate::embedder::RemoteEmbedder;
use crate::endpoint::{Authentication, Endpoint};
use crate::generator::RemoteGenerator;
use crate::util::loopback::is_loopback;

/// Every remote model behind one endpoint. Probing makes no request.
#[derive(Debug)]
pub struct HttpBackend {
    agent: Agent,
    base_url: String,
    credentials: Arc<dyn CredentialSource>,
    images_allowed: bool,
}

/// What a remote model needs for each request.
#[derive(Debug, Clone)]
pub(crate) struct RemoteModel {
    pub(crate) agent: Agent,
    pub(crate) url: String,
    pub(crate) name: String,
    pub(crate) credentials: Arc<dyn CredentialSource>,
    /// Pixels go only to a server on this machine.
    pub(crate) images_allowed: bool,
}

impl HttpBackend {
    pub fn new(endpoint: &Endpoint) -> Result<Self, InferError> {
        let credentials: Arc<dyn CredentialSource> = match &endpoint.authentication {
            Authentication::ApiKeyFromEnvironment { variable } => Arc::new(EnvironmentApiKey {
                variable: variable.clone(),
            }),
            Authentication::None | Authentication::MutualTls { .. } => Arc::new(NoCredentials),
        };
        Self::with_credentials(endpoint, credentials)
    }

    /// The same, with the `Authorization` header from `credentials`: the hook for a secrets
    /// proxy.
    pub fn with_credentials(
        endpoint: &Endpoint,
        credentials: Arc<dyn CredentialSource>,
    ) -> Result<Self, InferError> {
        let mut config = Agent::config_builder().http_status_as_error(false);
        if let Authentication::MutualTls {
            certificate,
            private_key,
            authority,
        } = &endpoint.authentication
        {
            config = config.tls_config(mutual_tls(certificate, private_key, authority)?);
        }
        Ok(Self {
            agent: Agent::new_with_config(config.build()),
            base_url: endpoint.base_url.trim_end_matches('/').to_owned(),
            credentials,
            images_allowed: is_loopback(&endpoint.base_url),
        })
    }

    fn remote_model(&self, spec: &LoadSpec<'_>, path: &str) -> Result<RemoteModel, InferError> {
        let Artifact::Remote { name } = spec.artifact else {
            return Err(InferError::Unavailable("a remote artifact".into()));
        };
        Ok(RemoteModel {
            agent: self.agent.clone(),
            url: format!("{}{path}", self.base_url),
            name: name.clone(),
            credentials: Arc::clone(&self.credentials),
            images_allowed: self.images_allowed,
        })
    }
}

impl RuntimeBackend for HttpBackend {
    fn runtime(&self) -> Runtime {
        Runtime::OpenAiCompatible
    }

    fn version(&self) -> String {
        format!("openai-compatible {}", self.base_url)
    }

    fn probe(&self, device: Device) -> bool {
        device == Device::Remote
    }

    fn load_generator(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Generate>, InferError> {
        Ok(Arc::new(RemoteGenerator::new(
            self.remote_model(spec, "/v1/chat/completions")?,
        )))
    }

    fn load_embedder(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Embed>, InferError> {
        let space = spec.embed_space().ok_or_else(|| {
            InferError::Unavailable(format!("the dimensions of {}", spec.model_id).into())
        })?;
        Ok(Arc::new(RemoteEmbedder::new(
            self.remote_model(spec, "/v1/embeddings")?,
            space,
            spec.model,
        )))
    }
}

fn mutual_tls(
    certificate: &Path,
    private_key: &Path,
    authority: &Path,
) -> Result<TlsConfig, InferError> {
    let chain: Vec<Certificate<'static>> = pem_items(certificate)?
        .into_iter()
        .filter_map(|item| match item {
            PemItem::Certificate(certificate) => Some(certificate),
            _ => None,
        })
        .collect();
    let key: PrivateKey<'static> = pem_items(private_key)?
        .into_iter()
        .find_map(|item| match item {
            PemItem::PrivateKey(key) => Some(key),
            _ => None,
        })
        .ok_or_else(|| unreadable(private_key, "no private key"))?;
    let authorities: Vec<Certificate<'static>> = pem_items(authority)?
        .into_iter()
        .filter_map(|item| match item {
            PemItem::Certificate(certificate) => Some(certificate),
            _ => None,
        })
        .collect();
    Ok(TlsConfig::builder()
        .client_cert(Some(ClientCert::new_with_certs(&chain, key)))
        .root_certs(RootCerts::new_with_certs(&authorities))
        .build())
}

fn pem_items(path: &Path) -> Result<Vec<PemItem<'static>>, InferError> {
    let pem = std::fs::read(path).map_err(|error| unreadable(path, error))?;
    ureq::tls::parse_pem(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| unreadable(path, error))
}

fn unreadable(path: &Path, error: impl std::fmt::Display) -> InferError {
    InferError::Unavailable(format!("the TLS file {} ({error})", path.display()).into())
}
