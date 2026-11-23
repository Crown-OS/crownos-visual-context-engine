use std::time::{Duration, Instant};

use context_infer::InferError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ureq::Agent;

use crate::credentials::CredentialSource;

const DEFAULT_RETRY: Duration = Duration::from_secs(1);

/// Posts `body` as compact JSON and reads the JSON answer, within `deadline`. 429 and 503 are `Busy`;
/// other errors of the server are `Backend`.
pub(crate) fn post_json<T: DeserializeOwned>(
    agent: &Agent,
    credentials: &dyn CredentialSource,
    url: &str,
    body: &impl Serialize,
    deadline: Instant,
) -> Result<T, InferError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(InferError::Timeout);
    }
    let mut request = agent
        .post(url)
        .config()
        .timeout_global(Some(remaining))
        .build();
    if let Some(authorization) = credentials.authorization()? {
        request = request.header("Authorization", authorization);
    }
    let body = serde_json::to_vec(body).map_err(|error| InferError::Backend(error.to_string()))?;
    let mut response = request
        .header("Content-Type", "application/json")
        .send(&body[..])
        .map_err(|error| match error {
            ureq::Error::Timeout(_) => InferError::Timeout,
            error => InferError::Backend(format!("{url}: {error}")),
        })?;
    let status = response.status().as_u16();
    if status == 429 || status == 503 {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|seconds| seconds.trim().parse().ok())
            .map_or(DEFAULT_RETRY, Duration::from_secs);
        return Err(InferError::Busy { retry_after });
    }
    if !(200..300).contains(&status) {
        let detail = response.body_mut().read_to_string().unwrap_or_default();
        return Err(InferError::Backend(format!(
            "{url} answered {status}: {detail}"
        )));
    }
    response
        .body_mut()
        .read_json()
        .map_err(|error| InferError::Backend(format!("{url}: reading the answer: {error}")))
}
