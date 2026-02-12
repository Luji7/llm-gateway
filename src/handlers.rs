use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::info;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::{Span, Tracer};

use crate::error::{map_downstream_error, AppError};
use crate::models::*;
use crate::streaming::stream_messages;
use crate::state::{AppState, InflightGuard};
use crate::translate::{anthropic_to_openai, openai_to_anthropic};
use crate::translate::openai_models_to_anthropic;

pub async fn post_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AnthropicRequest>,
) -> Result<axum::response::Response, AppError> {
    let _ = headers;
    let request_id = next_request_id();
    let start = Instant::now();
    let mut payload = payload;
    let model_before_map = payload.model.clone();
    if !state.config.models.allowlist.is_empty()
        && !state.config.models.allowlist.contains(&payload.model)
    {
        let err = AppError::invalid_request("model not in allowlist");
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &payload.model, start.elapsed().as_millis(), &err);
        return Err(err);
    }
    if state.config.models.blocklist.contains(&payload.model) {
        let err = AppError::invalid_request("model is blocked");
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &payload.model, start.elapsed().as_millis(), &err);
        return Err(err);
    }

    if let Some(mapped) = state.config.models.model_map.get(&payload.model) {
        payload.model = mapped.clone();
    }
    let openai_req = anthropic_to_openai(payload, &state.config).map_err(|e| {
        let err = AppError::from_translate(e);
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &model_before_map, start.elapsed().as_millis(), &err);
        err
    })?;
    let input_messages = serialize_json_for_trace(&openai_req.messages);
    let downstream_request = serialize_for_trace(&openai_req);

    let inflight = match state.inflight.clone().try_acquire_owned() {
        Ok(p) => InflightGuard::new(p, state.inflight_count.clone()),
        Err(_) => {
            let err = AppError::rate_limited("too many in-flight requests");
            let error_type = err.error_type.clone();
            state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
            log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &err);
            return Err(err);
        }
    };

    if openai_req.stream == Some(true) {
        let span = start_trace_span(
            &request_id,
            &openai_req.model,
            input_messages,
            downstream_request,
            None,
            None,
        );
        state.metrics.requests.add(1, &[KeyValue::new("stream", "true")]);
        if !state.config.observability.dump_downstream {
            info!(
                request_id = %request_id,
                model = %openai_req.model,
                "stream request accepted"
            );
        }
        return stream_messages(state, openai_req, inflight, request_id, start, span).await;
    }
    if state.config.observability.dump_downstream {
        info!(
            request_id = %request_id,
            "downstream request: {}",
            downstream_request
        );
    }
    state.metrics.requests.add(1, &[KeyValue::new("stream", "false")]);

    let resp = state
        .client
        .post(state.config.chat_completions_url())
        .header(CONTENT_TYPE, "application/json")
        .header(
            AUTHORIZATION,
            format!("Bearer {}", state.config.downstream.api_key),
        )
        .json(&openai_req)
        .send()
        .await
        .map_err(|e| {
        let err = AppError::api_error(format!("downstream request failed: {}", e));
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &err);
        err
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let mapped = map_downstream_error(status, &text);
        let error_type = mapped.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &mapped);
        return Err(mapped);
    }

    let raw_body = resp.text().await.map_err(|e| {
        let err = AppError::api_error(format!("invalid downstream response: {}", e));
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &err);
        err
    })?;

    if state.config.observability.dump_downstream {
        info!("downstream response: {}", raw_body);
    }

    let openai_resp: OpenAIResponse = serde_json::from_str(&raw_body).map_err(|e| {
        let err = AppError::api_error(format!("invalid downstream response: {}", e));
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &err);
        err
    })?;

    let downstream_response = truncate_for_trace(&raw_body);
    let output_messages = openai_output_messages(&openai_resp);
    let output_trace = serialize_json_for_trace(&output_messages);
    let mut span = start_trace_span(
        &request_id,
        &openai_req.model,
        input_messages,
        downstream_request,
        Some(output_trace),
        Some(downstream_response),
    );

    let anthropic_resp = openai_to_anthropic(openai_resp).map_err(|e| {
        let err = AppError::from_translate(e);
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &err);
        span.set_attribute(KeyValue::new("error.type", err.error_type.clone()));
        err
    })?;
    if state.config.observability.dump_downstream {
        if output_messages.as_array().map(|arr| arr.is_empty()).unwrap_or(false) {
            info!(
                request_id = %request_id,
                "upstream response has no output content"
            );
        }
    }
    if state.config.observability.dump_downstream {
        let upstream = serde_json::to_string(&anthropic_resp).unwrap_or_else(|_| "[unserializable]".to_string());
        info!(
            request_id = %request_id,
            "upstream response: {}",
            upstream
        );
    }

    state.metrics.latency_ms.record(
        start.elapsed().as_millis() as f64,
        &[KeyValue::new("stream", "false")],
    );
    info!(
        request_id = %request_id,
        model = %openai_req.model,
        latency_ms = start.elapsed().as_millis(),
        status = 200,
        "request completed"
    );
    tokio::spawn(async move {
        span.end();
    });
    Ok(Json(anthropic_resp).into_response())
}

pub async fn get_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<axum::response::Response, AppError> {
    let _ = headers;
    if let Some(override_models) = &state.config.models.models_override {
        let resp = AnthropicModelsResponse {
            data: override_models.clone(),
        };
        return Ok(Json(resp).into_response());
    }

    let resp = state
        .client
        .get(state.config.models_url())
        .header(
            AUTHORIZATION,
            format!("Bearer {}", state.config.downstream.api_key),
        )
        .send()
        .await
        .map_err(|e| AppError::api_error(format!("downstream request failed: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let mapped = map_downstream_error(status, &text);
        return Err(mapped);
    }

    let openai_resp: OpenAIModelsResponse = resp
        .json()
        .await
        .map_err(|e| AppError::api_error(format!("invalid downstream response: {}", e)))?;

    let anthropic_resp = openai_models_to_anthropic(openai_resp, &state.config.models.display_map)
        .map_err(AppError::from_translate)?;

    Ok(Json(anthropic_resp).into_response())
}

pub async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok"
    }))
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> String {
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("req-{}-{}", ts, seq)
}

fn log_error(request_id: &str, model: &str, latency_ms: u128, err: &AppError) {
    info!(
        request_id = %request_id,
        model = %model,
        latency_ms = latency_ms,
        status = err.status.as_u16(),
        error_type = %err.error_type,
        "request failed"
    );
}

fn start_trace_span(
    request_id: &str,
    model: &str,
    input_messages: String,
    downstream_request: String,
    output_messages: Option<String>,
    downstream_response: Option<String>,
) -> opentelemetry::global::BoxedSpan {
    let tracer = global::tracer("llm-gateway");
    let mut span = tracer.start("ai.gateway.request");
    span.set_attribute(KeyValue::new("request.id", request_id.to_string()));
    span.set_attribute(KeyValue::new("model", model.to_string()));
    span.set_attribute(KeyValue::new("input", input_messages));
    if let Some(output) = output_messages {
        span.set_attribute(KeyValue::new("output", output));
    }
    span.set_attribute(KeyValue::new("downstream.request", downstream_request));
    if let Some(resp) = downstream_response {
        span.set_attribute(KeyValue::new("downstream.response", resp));
    }
    span
}

fn serialize_for_trace<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(s) => s,
        Err(_) => "[unserializable]".to_string(),
    }
}

fn serialize_json_for_trace<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(s) => s,
        Err(_) => "[unserializable]".to_string(),
    }
}

fn truncate_for_trace(value: &str) -> String {
    value.to_string()
}


fn openai_output_messages(resp: &OpenAIResponse) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = resp
        .choices
        .iter()
        .map(|choice| {
            let mut obj = serde_json::Map::new();
            if let Some(tool_calls) = &choice.message.tool_calls {
                obj.insert("tool_calls".to_string(), serde_json::json!(tool_calls));
            }
            obj.insert("role".to_string(), serde_json::Value::String(choice.message.role.clone()));
            if let Some(reasoning) = &choice.message.reasoning_content {
                obj.insert("reasoning_content".to_string(), reasoning.clone());
            }
            if let Some(content) = &choice.message.content {
                obj.insert(
                    "content".to_string(),
                    serde_json::Value::String(content.clone()),
                );
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    serde_json::Value::Array(messages)
}
