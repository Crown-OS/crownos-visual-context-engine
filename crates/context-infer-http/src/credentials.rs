use context_infer::InferError;

/// Gives the `Authorization` header for each request. The default reads an environment
/// variable; crownos-secrets-manager can supply its own source later, so that the key never
/// enters this process.
pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    fn authorization(&self) -> Result<Option<String>, InferError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentApiKey {
    pub variable: String,
}

impl CredentialSource for EnvironmentApiKey {
    fn authorization(&self) -> Result<Option<String>, InferError> {
        std::env::var(&self.variable)
            .map(|key| Some(format!("Bearer {key}")))
            .map_err(|_| {
                InferError::Unavailable(format!("the API key in ${}", self.variable).into())
            })
    }
}

/// No `Authorization` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NoCredentials;

impl CredentialSource for NoCredentials {
    fn authorization(&self) -> Result<Option<String>, InferError> {
        Ok(None)
    }
}
