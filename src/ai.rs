use crate::{
    error::{AppError, Result},
    state::AppState,
};
use serde_json::Value;
use std::time::Duration;

const MAX_ERROR: usize = 300;

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
                let message: String = text.trim().chars().take(MAX_ERROR).collect();
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

pub(crate) fn parse_payload(reply: &Value) -> Result<Value> {
    if let Some(error) = reply.get("error").filter(|error| !error.is_null()) {
        let message: String = error.to_string().chars().take(MAX_ERROR).collect();
        return Err(AppError::Internal(format!("AI 接口错误: {message}").into()));
    }
    if reply["status"] != "completed" {
        return Err(AppError::Internal(
            format!("AI 响应未完成: {}", reply["status"]).into(),
        ));
    }
    let text = reply["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|output| output["type"] == "message")
        .filter_map(|output| output["content"].as_array())
        .flatten()
        .find(|content| content["type"] == "output_text")
        .and_then(|content| content["text"].as_str())
        .ok_or_else(|| AppError::Internal("AI 响应没有文本输出".into()))?;
    // Some providers wrap the JSON object in prose, even in strict mode.
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text,
    };
    serde_json::from_str(json).map_err(|_| AppError::Internal("AI 响应不是有效的 JSON".into()))
}
