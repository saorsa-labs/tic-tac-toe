use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};

use super::{X0xClient, X0xClientError};

impl X0xClient {
    /// Authenticated `POST` with an explicit bounded deadline for daemon
    /// operations that exceed the ordinary REST request deadline.
    pub(crate) async fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, X0xClientError> {
        let resolved = self.resolve()?;
        let url = format!("{}{}", resolved.api_base, path);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&resolved.token)
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|error| X0xClientError::Transport(format!("POST {path}: {error}")))?;
        Self::decode_json::<T>(response, path).await
    }
}
