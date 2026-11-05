use crate::{
    error::{AppError, Result},
    state::AppState,
};
use serde_json::{Value, json};
use std::time::Duration;

const MAX_ERROR: usize = 300;

/// Shared transport for collection and daily analysis. Their outer task deadlines also
/// bound retries; non-transient HTTP errors return immediately. A grok request that still
/// gets no completed answer is asked once more of `AI_FALLBACK_MODEL`, when one is set.
pub(crate) async fn request(state: &AppState, body: Value) -> Result<Value> {
    let answer = send(state, &body).await;
    if answer.as_ref().is_ok_and(completed) {
        return answer;
    }
    let Some(spare) = fallback(state.config.ai_fallback_model.as_deref(), &body) else {
        return answer;
    };
    tracing::debug!(
        failure = %answer.map_or_else(|error| error.to_string(), |reply| reply["status"].to_string()),
        model = %spare["model"],
        "grok did not answer; asking the fallback model"
    );
    send(state, &spare).await
}

/// Whether a reply finished without an error, as `parse_payload` requires.
fn completed(reply: &Value) -> bool {
    reply["status"] == "completed" && reply.get("error").is_none_or(Value::is_null)
}

/// The request as the fallback model takes it: without X search and grok's picture options,
/// which only grok has. None without a fallback, for a request not grok's, or for one that
/// searches X alone.
fn fallback(model: Option<&str>, body: &Value) -> Option<Value> {
    let model = model?;
    if !body["model"].as_str()?.starts_with("grok") {
        return None;
    }
    let mut spare = body.clone();
    spare["model"] = json!(model);
    if let Some(tools) = spare.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| tool["type"] != "x_search");
        if tools.is_empty() {
            return None;
        }
        for tool in tools.iter_mut().filter_map(Value::as_object_mut) {
            tool.retain(|key, _| key == "type");
        }
    }
    Some(spare)
}

/// The request to the model it names alone, retried as above but never handed to another: for
/// an answer only that model can give.
pub(crate) async fn send(state: &AppState, body: &Value) -> Result<Value> {
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
            .json(body)
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

/// A strict JSON Schema object: every property is required and no other is allowed.
pub(crate) fn object(properties: Value) -> Value {
    let required: Vec<_> = properties
        .as_object()
        .expect("schema properties")
        .keys()
        .cloned()
        .collect();
    json!({"type": "object", "additionalProperties": false, "required": required, "properties": properties})
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
        // Search responses can include a draft before the final assistant message.
        .rev()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_final_result_after_search_drafts() {
        let message =
            |text: &str| json!({"type":"message","content":[{"type":"output_text","text":text}]});
        let mut reply = json!({"status":"completed","output":[
            message(r#"{"image":null}"#),
            {"type":"web_search_call"},
            message(r#"{"image":"https://images.example/cover.jpg"}"#)
        ]});
        assert_eq!(
            parse_payload(&reply).unwrap()["image"],
            "https://images.example/cover.jpg"
        );
        reply["output"][2] = message("An incomplete final answer");
        assert!(parse_payload(&reply).is_err());
    }

    #[test]
    fn the_fallback_asks_without_what_only_grok_has() {
        let tools = json!([
            { "type": "web_search", "enable_image_search": true, "enable_image_understanding": true },
            { "type": "x_search" }
        ]);
        let body = json!({ "model": "grok-4.7", "tools": tools, "input": "栏目方向" });
        let spare = fallback(Some("gpt-6.1-terra"), &body).unwrap();
        assert_eq!(
            spare,
            json!({ "model": "gpt-6.1-terra", "tools": [{ "type": "web_search" }], "input": "栏目方向" })
        );
        // A request without tools keeps none; one that searches X alone, one not grok's and any
        // without a fallback model aren't asked again.
        let plain = json!({ "model": "grok-4.7", "input": "导读" });
        assert_eq!(
            fallback(Some("gpt-6.1-terra"), &plain).unwrap(),
            json!({ "model": "gpt-6.1-terra", "input": "导读" })
        );
        let x_alone = json!({ "model": "grok-4.7", "tools": [{ "type": "x_search" }] });
        assert!(fallback(Some("gpt-6.1-terra"), &x_alone).is_none());
        // A request that also looks at X's pictures is asked with what the fallback can do.
        let x_pictures = json!({ "model": "grok-4.7", "tools": [
            { "type": "web_search" }, { "type": "x_search", "enable_image_understanding": true }
        ] });
        assert_eq!(
            fallback(Some("gpt-6.1-terra"), &x_pictures).unwrap()["tools"],
            json!([{ "type": "web_search" }])
        );
        assert!(fallback(Some("gpt-6.1-terra"), &json!({ "model": "gpt-6-astra" })).is_none());
        assert!(fallback(None, &body).is_none());
        assert!(completed(&json!({ "status": "completed" })));
        assert!(!completed(&json!({ "status": "incomplete" })));
        assert!(!completed(
            &json!({ "status": "completed", "error": { "message": "x" } })
        ));
    }
}
