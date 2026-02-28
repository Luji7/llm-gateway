use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::info;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::{Span, Tracer};

use crate::error::{map_downstream_error, AppError};
use crate::models::*;
use crate::streaming::{stream_anthropic_passthrough, stream_messages};
use crate::state::{AppState, InflightGuard};
use crate::translate::{anthropic_to_openai, openai_to_anthropic};
use crate::translate::openai_models_to_anthropic;
use crate::audit_log::{AuditContext, AuditMeta, headers_to_map, now_ms};

pub async fn post_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<axum::response::Response, AppError> {
    let request_id = next_request_id();
    let start = Instant::now();
    let payload = payload;
    let upstream_payload = payload.clone();
    let model = extract_model(&payload)?;
    let model_before_map = model.clone();
    if !state.config.models.allowlist.is_empty()
        && !state.config.models.allowlist.contains(&model)
    {
        let err = AppError::invalid_request("model not in allowlist");
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &model, start.elapsed().as_millis(), &err);
        return Err(err);
    }
    if state.config.models.blocklist.contains(&model) {
        let err = AppError::invalid_request("model is blocked");
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &model, start.elapsed().as_millis(), &err);
        return Err(err);
    }

    let stream = extract_stream(&payload);
    let input_messages = extract_messages_for_trace(&payload);
    let downstream_request = serialize_for_trace(&payload);

    let inflight = match state.inflight.clone().try_acquire_owned() {
        Ok(p) => InflightGuard::new(p, state.inflight_count.clone()),
        Err(_) => {
            let err = AppError::rate_limited("too many in-flight requests");
            let error_type = err.error_type.clone();
            state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
            log_error(&request_id, &model, start.elapsed().as_millis(), &err);
            return Err(err);
        }
    };

    if state.config.forward_mode() == "passthrough" {
        let audit_ctx = build_audit_context(
            &state,
            &request_id,
            "/v1/messages",
            "POST",
            &headers,
            upstream_payload.clone(),
            Some(model.clone()),
            stream,
        );
        if state.config.observability.dump_downstream {
            info!(
                request_id = %request_id,
                "upstream request headers: {}",
                headers_for_trace(&headers)
            );
            info!(
                request_id = %request_id,
                "upstream request body: {}",
                truncate_for_trace(&downstream_request)
            );
        }
        let forward_headers = build_passthrough_headers(&headers, &state.config.downstream.base_url);
        if stream == Some(true) {
            if state.config.observability.dump_downstream {
                info!(
                    request_id = %request_id,
                    "downstream request headers: {}",
                    headers_for_trace(&forward_headers)
                );
                info!(
                    request_id = %request_id,
                    "downstream request body: {}",
                    truncate_for_trace(&downstream_request)
                );
            }
            let span = start_trace_span(
                &request_id,
                &model,
                input_messages,
                downstream_request,
                None,
                None,
            );
            state.metrics.requests.add(1, &[KeyValue::new("stream", "true")]);
            if !state.config.observability.dump_downstream {
                info!(
                    request_id = %request_id,
                    model = %model,
                    "stream request accepted"
                );
            }
            return stream_anthropic_passthrough(
                state,
                payload,
                forward_headers,
                model,
                audit_ctx,
                inflight,
                request_id,
                start,
                span,
            )
            .await;
        }

        if state.config.observability.dump_downstream {
            info!(
                request_id = %request_id,
                "downstream request: {}",
                downstream_request
            );
            info!(
                request_id = %request_id,
                "downstream request headers: {}",
                headers_for_trace(&forward_headers)
            );
            info!(
                request_id = %request_id,
                "downstream request url: {}",
                state.config.anthropic_messages_url()
            );
        }
        state.metrics.requests.add(1, &[KeyValue::new("stream", "false")]);

        let span = start_trace_span(
            &request_id,
            &model,
            input_messages,
            downstream_request,
            None,
            None,
        );

        let request = state
            .client
            .post(state.config.anthropic_messages_url())
            .headers(forward_headers);
        let resp = request.json(&payload).send().await.map_err(|e| {
                let err = AppError::api_error(format!("downstream request failed: {}", e));
                let error_type = err.error_type.clone();
                state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
                log_error(&request_id, &model, start.elapsed().as_millis(), &err);
                err
            })?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let raw_body = resp.bytes().await.map_err(|e| {
            let err = AppError::api_error(format!("invalid downstream response: {}", e));
            let error_type = err.error_type.clone();
            state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
            log_error(&request_id, &model, start.elapsed().as_millis(), &err);
            err
        })?;

        if state.config.observability.dump_downstream {
            info!(
                request_id = %request_id,
                "downstream response headers: {}",
                headers_for_trace(&headers)
            );
            if let Ok(text) = std::str::from_utf8(&raw_body) {
                info!("downstream response: {}", text);
            }
        }

        let mut span = span;
        span.set_attribute(KeyValue::new(
            "downstream.response",
            truncate_for_trace(&String::from_utf8_lossy(&raw_body)),
        ));
        state.metrics.latency_ms.record(
            start.elapsed().as_millis() as f64,
            &[KeyValue::new("stream", "false")],
        );
        info!(
            request_id = %request_id,
            model = %model,
            latency_ms = start.elapsed().as_millis(),
            status = status.as_u16(),
            "request completed"
        );
        tokio::spawn(async move {
            span.end();
        });

        if let Some((logger, ctx)) = state.audit_logger.clone().zip(audit_ctx) {
            let (body_value, parse_error) = parse_body_value(&raw_body);
            let record = ctx.finish(
                status.as_u16(),
                headers_to_map(&headers),
                body_value,
                parse_error,
                false,
                now_ms(),
            );
            logger.push(record).await;
        }

        return Ok(response_from_bytes(status, headers.get(CONTENT_TYPE), raw_body));
    }

    let mut anthropic_req: AnthropicRequest = serde_json::from_value(payload).map_err(|e| {
        let err = AppError::invalid_request(format!("invalid request: {}", e));
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &model_before_map, start.elapsed().as_millis(), &err);
        err
    })?;
    if let Some(mapped) = state.config.models.model_map.get(&model) {
        anthropic_req.model = mapped.clone();
    }

    let openai_req = anthropic_to_openai(anthropic_req, &state.config).map_err(|e| {
        let err = AppError::from_translate(e);
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &model_before_map, start.elapsed().as_millis(), &err);
        err
    })?;
    let input_messages = serialize_json_for_trace(&openai_req.messages);
    let downstream_request = serialize_for_trace(&openai_req);

    if openai_req.stream == Some(true) {
        let audit_ctx = build_audit_context(
            &state,
            &request_id,
            "/v1/messages",
            "POST",
            &headers,
            upstream_payload.clone(),
            Some(openai_req.model.clone()),
            openai_req.stream,
        );
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
        return stream_messages(
            state,
            openai_req,
            inflight,
            request_id,
            start,
            span,
            audit_ctx,
        )
        .await;
    }
    if state.config.observability.dump_downstream {
        info!(
            request_id = %request_id,
            "downstream request: {}",
            downstream_request
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!(
                "Bearer {}",
                state.config.downstream.api_key.as_deref().unwrap_or_default()
            ))
            .unwrap_or_else(|_| HeaderValue::from_static("[invalid]")),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        info!(
            request_id = %request_id,
            "downstream request headers: {}",
            headers_for_trace(&headers)
        );
        info!(
            request_id = %request_id,
            "downstream request url: {}",
            state.config.chat_completions_url()
        );
    }
    state.metrics.requests.add(1, &[KeyValue::new("stream", "false")]);

    let resp = state
        .client
        .post(state.config.chat_completions_url())
        .header(CONTENT_TYPE, "application/json")
        .header(
            AUTHORIZATION,
            format!(
                "Bearer {}",
                state.config.downstream.api_key.as_deref().unwrap_or_default()
            ),
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

    let headers = resp.headers().clone();
    let raw_body = resp.text().await.map_err(|e| {
        let err = AppError::api_error(format!("invalid downstream response: {}", e));
        let error_type = err.error_type.clone();
        state.metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
        log_error(&request_id, &openai_req.model, start.elapsed().as_millis(), &err);
        err
    })?;

    if state.config.observability.dump_downstream {
        info!(
            request_id = %request_id,
            "downstream response headers: {}",
            headers_for_trace(&headers)
        );
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

    if let Some(logger) = state.audit_logger.clone() {
        let ctx = build_audit_context(
            &state,
            &request_id,
            "/v1/messages",
            "POST",
            &headers,
            upstream_payload.clone(),
            Some(openai_req.model.clone()),
            openai_req.stream,
        );
        if let Some(ctx) = ctx {
            let mut response_headers = HeaderMap::new();
            response_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let record = ctx.finish(
                200,
                headers_to_map(&response_headers),
                serde_json::to_value(&anthropic_resp).unwrap_or(Value::Null),
                false,
                false,
                now_ms(),
            );
            logger.push(record).await;
        }
    }
    Ok(Json(anthropic_resp).into_response())
}

pub async fn get_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<axum::response::Response, AppError> {
    if let Some(override_models) = &state.config.models.models_override {
        let resp = AnthropicModelsResponse {
            data: override_models.clone(),
        };
        return Ok(Json(resp).into_response());
    }

    if state.config.forward_mode() == "passthrough" {
        let audit_ctx = build_audit_context(
            &state,
            "models",
            "/v1/models",
            "GET",
            &headers,
            Value::Null,
            None,
            None,
        );
        if state.config.observability.dump_downstream {
            info!(
                request_id = "models",
                "upstream request headers: {}",
                headers_for_trace(&headers)
            );
        }
        let forward_headers = build_passthrough_headers(&headers, &state.config.downstream.base_url);
        let request = state
            .client
            .get(state.config.anthropic_models_url())
            .headers(forward_headers);
        let resp = request
            .send()
            .await
            .map_err(|e| AppError::api_error(format!("downstream request failed: {}", e)))?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let raw_body = resp
            .bytes()
            .await
            .map_err(|e| AppError::api_error(format!("invalid downstream response: {}", e)))?;
        if let Some((logger, ctx)) = state.audit_logger.clone().zip(audit_ctx) {
            let (body_value, parse_error) = parse_body_value(&raw_body);
            let record = ctx.finish(
                status.as_u16(),
                headers_to_map(&headers),
                body_value,
                parse_error,
                false,
                now_ms(),
            );
            logger.push(record).await;
        }
        return Ok(response_from_bytes(
            status,
            headers.get(CONTENT_TYPE),
            raw_body,
        ));
    }

    let resp = state
        .client
        .get(state.config.models_url())
        .header(
            AUTHORIZATION,
            format!(
                "Bearer {}",
                state.config.downstream.api_key.as_deref().unwrap_or_default()
            ),
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

    if let Some(logger) = state.audit_logger.clone() {
        let ctx = build_audit_context(
            &state,
            "models",
            "/v1/models",
            "GET",
            &headers,
            Value::Null,
            None,
            None,
        );
        if let Some(ctx) = ctx {
            let mut response_headers = HeaderMap::new();
            response_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let record = ctx.finish(
                200,
                headers_to_map(&response_headers),
                serde_json::to_value(&anthropic_resp).unwrap_or(Value::Null),
                false,
                false,
                now_ms(),
            );
            logger.push(record).await;
        }
    }
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

fn build_audit_context(
    state: &AppState,
    request_id: &str,
    route: &str,
    method: &str,
    headers: &HeaderMap,
    body: Value,
    model: Option<String>,
    stream: Option<bool>,
) -> Option<AuditContext> {
    if !state.config.observability.audit_log.enabled {
        return None;
    }
    if state.audit_logger.is_none() {
        return None;
    }
    Some(AuditContext {
        ts_start_ms: now_ms(),
        request_id: request_id.to_string(),
        route: route.to_string(),
        mode: state.config.forward_mode().to_string(),
        method: method.to_string(),
        request_headers: headers_to_map(headers),
        request_body: body,
        meta: AuditMeta {
            model,
            stream,
            body_truncated: false,
            body_parse_error: false,
        },
    })
}

fn parse_body_value(bytes: &[u8]) -> (Value, bool) {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => (value, false),
        Err(_) => (Value::Null, true),
    }
}

fn headers_for_trace(headers: &HeaderMap) -> String {
    let mut out = serde_json::Map::new();
    for (name, value) in headers.iter() {
        let value = value.to_str().unwrap_or("[invalid]");
        out.insert(name.to_string(), serde_json::Value::String(value.to_string()));
    }
    serde_json::Value::Object(out)
        .to_string()
}

fn extract_model(payload: &Value) -> Result<String, AppError> {
    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::invalid_request("model is required"))?;
    if model.is_empty() {
        return Err(AppError::invalid_request("model is required"));
    }
    Ok(model)
}

fn extract_stream(payload: &Value) -> Option<bool> {
    payload.get("stream").and_then(|v| v.as_bool())
}

fn extract_messages_for_trace(payload: &Value) -> String {
    let messages = payload.get("messages").cloned().unwrap_or(Value::Null);
    serialize_json_for_trace(&messages)
}

fn response_from_bytes(
    status: StatusCode,
    content_type: Option<&HeaderValue>,
    body: Bytes,
) -> axum::response::Response {
    let mut builder = axum::response::Response::builder().status(status);
    if let Some(ct) = content_type {
        builder = builder.header(CONTENT_TYPE, ct);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| axum::response::Response::builder().status(status).body(Body::empty()).unwrap())
}

fn build_passthrough_headers(incoming: &HeaderMap, base_url: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in incoming.iter() {
        let key = name.as_str();
        if key == "host" || key == "content-length" {
            continue;
        }
        headers.insert(name.clone(), value.clone());
    }
    if let Ok(url) = Url::parse(base_url) {
        if let Some(host) = url.host_str() {
            let host_value = match url.port() {
                Some(port) => format!("{}:{}", host, port),
                None => host.to_string(),
            };
            if let Ok(value) = HeaderValue::from_str(&host_value) {
                headers.insert("host", value);
            }
        }
    }
    headers
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Bytes, routing::post, Router};
    use axum::response::Response;
    use http_body_util::BodyExt;
    use std::collections::{HashMap, HashSet};
    use std::convert::Infallible;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio_stream::wrappers::ReceiverStream;
    use crate::config::Config;
    use crate::metrics::init_metrics_noop;
    use crate::tracing_otlp::init_tracer_noop;

    #[derive(Debug)]
    struct Capture {
        headers: HeaderMap,
        body: Value,
    }

    async fn spawn_upstream(app: Router) -> Result<String, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok(format!("http://{}", addr))
    }

    fn test_state(base_url: String, model_map: HashMap<String, String>) -> AppState {
        let inflight_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let metrics = init_metrics_noop(inflight_count.clone());
        let config = Config {
            server: crate::config::ServerConfig {
                bind_addr: "127.0.0.1:0".to_string(),
            },
            downstream: crate::config::DownstreamConfig {
                base_url,
                api_key: Some("sk-test".to_string()),
                anthropic_version: Some("2023-06-01".to_string()),
                anthropic_beta: None,
                connect_timeout_ms: 5000,
                read_timeout_ms: 30000,
                pool_max_idle_per_host: 8,
            },
            anthropic: crate::config::AnthropicConfig {
                forward_mode: "passthrough".to_string(),
            },
            models: crate::config::ModelsConfig {
                model_map,
                display_map: HashMap::new(),
                allowlist: HashSet::new(),
                blocklist: HashSet::new(),
                thinking_map: HashMap::new(),
                output_strict: true,
                allow_images: true,
                document_policy: "reject".to_string(),
                models_override: None,
            },
            limits: crate::config::LimitsConfig { max_inflight: 8 },
            observability: crate::config::ObservabilityConfig {
                service_name: "llm-gateway".to_string(),
                dump_downstream: false,
                audit_log: crate::config::AuditLogConfig::default(),
                logging: crate::config::LoggingConfig::default(),
                otlp_grpc: crate::config::OtlpGrpcConfig::default(),
                otlp_http: crate::config::OtlpHttpConfig::default(),
                exporters: crate::config::ExportersConfig::default(),
            },
        };
        let tracer = init_tracer_noop(config.observability.service_name.clone());
        AppState {
            client: reqwest::Client::builder().build().unwrap(),
            stream_client: reqwest::Client::builder().build().unwrap(),
            config: config.clone(),
            inflight: std::sync::Arc::new(tokio::sync::Semaphore::new(config.limits.max_inflight)),
            inflight_count,
            metrics,
            audit_logger: None,
            _tracer_provider: tracer,
        }
    }

    #[tokio::test]
    async fn passthrough_non_stream_forwards_body_and_headers() {
        let captured: Arc<Mutex<Option<Capture>>> = Arc::new(Mutex::new(None));
        let captured_handler = captured.clone();
        let response_json = serde_json::json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type":"text","text":"ok"}],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
        });
        let response_clone = response_json.clone();
        let app = Router::new().route(
            "/v1/messages",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = captured_handler.clone();
                let response = response_clone.clone();
                async move {
                    *captured.lock().await = Some(Capture { headers, body });
                    Json(response)
                }
            }),
        );
        let base_url = match spawn_upstream(app).await {
            Ok(url) => url,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(err) => panic!("spawn upstream failed: {}", err),
        };

        let state = test_state(
            base_url,
            HashMap::from([("claude-opus".to_string(), "mapped-model".to_string())]),
        );
        let payload = serde_json::json!({
            "model": "claude-opus",
            "max_tokens": 8,
            "messages": [{"role":"user","content":"hi"}]
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-upstream"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        let resp = post_messages(State(state), headers, Json(payload))
            .await
            .expect("response ok");

        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(parsed, response_json);

        let capture = captured.lock().await.take().expect("capture");
        assert_eq!(
            capture.headers.get("x-api-key").unwrap(),
            "sk-upstream"
        );
        assert_eq!(
            capture.headers.get("anthropic-version").unwrap(),
            "2023-06-01"
        );
        let host = capture.headers.get("host").and_then(|v| v.to_str().ok());
        assert!(host.is_some());
        assert!(capture.headers.get(AUTHORIZATION).is_none());
        assert_eq!(
            capture.body.get("model").and_then(|v| v.as_str()),
            Some("claude-opus")
        );
    }

    #[tokio::test]
    async fn passthrough_error_status_transparent() {
        let error_json = serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "bad key"}
        });
        let error_clone = error_json.clone();
        let app = Router::new().route(
            "/v1/messages",
            post(move || {
                let err = error_clone.clone();
                async move { (StatusCode::UNAUTHORIZED, Json(err)) }
            }),
        );
        let base_url = match spawn_upstream(app).await {
            Ok(url) => url,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(err) => panic!("spawn upstream failed: {}", err),
        };

        let state = test_state(base_url, HashMap::new());
        let payload = serde_json::json!({
            "model": "claude-opus",
            "max_tokens": 8,
            "messages": [{"role":"user","content":"hi"}]
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-upstream"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        let resp = post_messages(State(state), headers, Json(payload))
            .await
            .expect("response ok");

        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(parsed, error_json);
    }

    #[tokio::test]
    async fn passthrough_stream_forwards_sse() {
        let app = Router::new().route(
            "/v1/messages",
            post(|| async move {
                let chunks = vec![
                    Ok::<Bytes, Infallible>(Bytes::from("event: message_start\n\n")),
                    Ok::<Bytes, Infallible>(Bytes::from("data: test\n\n")),
                ];
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, Infallible>>(4);
                tokio::spawn(async move {
                    for chunk in chunks {
                        let _ = tx.send(chunk).await;
                    }
                });
                let body = axum::body::Body::from_stream(ReceiverStream::new(rx));
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "text/event-stream")
                    .body(body)
                    .unwrap()
            }),
        );
        let base_url = match spawn_upstream(app).await {
            Ok(url) => url,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(err) => panic!("spawn upstream failed: {}", err),
        };

        let state = test_state(base_url, HashMap::new());
        let payload = serde_json::json!({
            "model": "claude-opus",
            "max_tokens": 8,
            "stream": true,
            "messages": [{"role":"user","content":"hi"}]
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-upstream"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        let resp = post_messages(State(state), headers, Json(payload))
            .await
            .expect("response ok");

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert_eq!(text, "event: message_start\n\ndata: test\n\n");
    }
}
