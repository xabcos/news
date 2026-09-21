use crate::{
    error::{AppError, Result},
    state::AppState,
};
use serde_json::Value;
use std::time::Duration;

/// Shared transport for collection and daily analysis. Their outer task deadlines also
/// bound retries; non-transient HTTP errors return immediately.
pub(crate) async fn request(state: &AppState, body: Value) -> Result<Value> {
    let url = format!(
        "{}/v1/responses",
        state.config.ai_base_url.as_str().trim_end_matches('/')
    );
    let mut delays = [10, 30].into_iter();
    loop {
        let failure = match state
            .http
            .post(&url)
            .bearer_auth(&state.config.ai_api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(response.json().await?),
            Ok(response)
                if response.status().as_u16() == 429 || response.status().is_server_error() =>
            {
                response.status().to_string()
            }
            Ok(response) => {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                let message: String = text.trim().chars().take(300).collect();
                return Err(AppError::Internal(format!("{status}: {message}").into()));
            }
            Err(error) => error.to_string(),
        };
        let Some(seconds) = delays.next() else {
            return Err(AppError::Internal(failure.into()));
        };
        tracing::debug!(%failure, seconds, model = body["model"].as_str(), "retrying AI request");
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }
}
