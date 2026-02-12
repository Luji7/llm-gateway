use axum::{
    body::Bytes,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;
use opentelemetry::KeyValue;
use opentelemetry::trace::Span;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::{map_downstream_error, AppError};
use crate::models::{AnthropicUsage, OpenAIRequest, OpenAIStreamChunk};
use crate::state::{AppState, InflightGuard};

struct StreamState {
    started: bool,
    message_id: Option<String>,
    model: Option<String>,
    next_index: u32,
    text_block_index: Option<u32>,
    thinking_block_index: Option<u32>,
    tool_calls: HashMap<u32, ToolCallState>,
    output_text: String,
    reasoning_text: String,
    reasoning_signature: Option<String>,
}

struct ToolCallState {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    block_index: u32,
    started: bool,
    stopped: bool,
}

pub async fn stream_messages(
    state: AppState,
    openai_req: OpenAIRequest,
    guard: InflightGuard,
    request_id: String,
    start: Instant,
    span: opentelemetry::global::BoxedSpan,
) -> Result<Response, AppError> {
    let _ = request_id;
    let span = span;
    if state.config.observability.dump_downstream {
        let body = serde_json::to_string(&openai_req).unwrap_or_else(|_| "[unserializable]".to_string());
        tracing::info!(
            request_id = %request_id,
            "downstream request: {}",
            body
        );
    }
    let resp = state
        .stream_client
        .post(state.config.chat_completions_url())
        .header(CONTENT_TYPE, "application/json")
        .header(
            AUTHORIZATION,
            format!("Bearer {}", state.config.downstream.api_key),
        )
        .json(&openai_req)
        .send()
        .await
        .map_err(|e| AppError::api_error(format!("downstream request failed: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let mapped = map_downstream_error(status, &text);
        return Err(mapped);
    }

    let mut stream = resp.bytes_stream();
    let (tx, rx) = mpsc::channel::<Result<Bytes, std::convert::Infallible>>(64);

    let metrics = state.metrics.clone();
    let dump_downstream = state.config.observability.dump_downstream;
    let model = openai_req.model.clone();
    tokio::spawn(async move {
        let _guard = guard;
        let mut span = span;
        let mut buffer = String::new();
        let mut response_trace = String::new();
        let mut state = StreamState {
            started: false,
            message_id: None,
            model: None,
            next_index: 0,
            text_block_index: None,
            thinking_block_index: None,
            tool_calls: HashMap::new(),
            output_text: String::new(),
            reasoning_text: String::new(),
            reasoning_signature: None,
        };

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(bytes) => bytes,
                Err(err) => {
                    let err = AppError::api_error(format!("stream error: {}", err));
                    let error_type = err.error_type.clone();
                    metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
                    span.set_attribute(KeyValue::new("error.type", err.error_type.clone()));
                    let _ = tx.send(Ok(Bytes::from(error_event(err)))).await;
                    span.end();
                    return;
                }
            };

            let text = String::from_utf8_lossy(&chunk);
            buffer.push_str(&text);

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim_end_matches('\r').to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() || !line.starts_with("data:") {
                    continue;
                }

                let data = line.trim_start_matches("data:").trim();
                if dump_downstream {
                    tracing::info!(
                        request_id = %request_id,
                        "downstream stream chunk: {}",
                        data
                    );
                }
                append_trace(&mut response_trace, data);
                if data == "[DONE]" {
                    if let Err(err) = flush_open_blocks(&mut state, &tx).await {
                        let error_type = err.error_type.clone();
                        metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
                        span.set_attribute(KeyValue::new("error.type", err.error_type.clone()));
                        let _ = tx.send(Ok(Bytes::from(error_event(err)))).await;
                        if dump_downstream {
                            if let Some(upstream) = stream_upstream_response(&state) {
                                tracing::info!(
                                    request_id = %request_id,
                                    "upstream response: {}",
                                    upstream
                                );
                            }
                            tracing::info!(
                                request_id = %request_id,
                                "downstream response: {}",
                                response_trace
                            );
                        }
                        span.end();
                        return;
                    }
                    let _ = tx
                        .send(Ok(Bytes::from(sse_event(
                            "message_stop",
                            json!({"type":"message_stop"}),
                        ))))
                        .await;
                    metrics.latency_ms.record(
                        start.elapsed().as_millis() as f64,
                        &[KeyValue::new("stream", "true")],
                    );
                    if let Some(output) = stream_output_messages(&state) {
                        let output = serialize_json_for_trace(&output);
                        span.set_attribute(KeyValue::new("output", output));
                    } else if dump_downstream {
                        tracing::info!(
                            request_id = %request_id,
                            "upstream response has no output content"
                        );
                    }
                    if dump_downstream {
                        if let Some(upstream) = stream_upstream_response(&state) {
                            tracing::info!(
                                request_id = %request_id,
                                "upstream response: {}",
                                upstream
                            );
                        }
                        tracing::info!(
                            request_id = %request_id,
                            "downstream response: {}",
                            response_trace
                        );
                    }
                    span.set_attribute(KeyValue::new(
                        "downstream.response",
                        response_trace.clone(),
                    ));
                    span.end();
                    return;
                }

                let parsed: OpenAIStreamChunk = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(err) => {
                    let err = AppError::api_error(format!("invalid stream chunk: {}", err));
                    let error_type = err.error_type.clone();
                    metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
                    span.set_attribute(KeyValue::new("error.type", err.error_type.clone()));
                    let _ = tx.send(Ok(Bytes::from(error_event(err)))).await;
                    span.end();
                    return;
                }
                };

                if let Err(err) = handle_openai_chunk(parsed, &mut state, &tx).await {
                    let error_type = err.error_type.clone();
                    metrics.errors.add(1, &[KeyValue::new("type", error_type)]);
                    span.set_attribute(KeyValue::new("error.type", err.error_type.clone()));
                    let _ = tx.send(Ok(Bytes::from(error_event(err)))).await;
                    if let Some(output) = stream_output_messages(&state) {
                        let output = serialize_json_for_trace(&output);
                        span.set_attribute(KeyValue::new("output", output));
                    } else if dump_downstream {
                        tracing::info!(
                            request_id = %request_id,
                            "upstream response has no output content"
                        );
                    }
                    if dump_downstream {
                        if let Some(upstream) = stream_upstream_response(&state) {
                            tracing::info!(
                                request_id = %request_id,
                                "upstream response: {}",
                                upstream
                            );
                        }
                        tracing::info!(
                            request_id = %request_id,
                            "downstream response: {}",
                            response_trace
                        );
                    }
                    span.set_attribute(KeyValue::new(
                        "downstream.response",
                        response_trace.clone(),
                    ));
                    span.end();
                    return;
                }
            }
        }
        let _ = model;
        let _ = request_id;
    });

    let body_stream = ReceiverStream::new(rx);
    let body = axum::body::Body::from_stream(body_stream);
    Ok((StatusCode::OK, body).into_response())
}

async fn handle_openai_chunk(
    parsed: OpenAIStreamChunk,
    state: &mut StreamState,
    tx: &mpsc::Sender<Result<Bytes, std::convert::Infallible>>,
) -> Result<(), AppError> {
    if !state.started {
        state.started = true;
        state.message_id = parsed.id.clone();
        state.model = parsed.model.clone();

        let message = json!({
            "id": state.message_id.clone().unwrap_or_else(|| "msg_stream".to_string()),
            "type": "message",
            "role": "assistant",
            "content": [],
            "usage": usage_zero(),
        });
        let _ = tx
            .send(Ok(Bytes::from(sse_event(
                "message_start",
                json!({"type":"message_start","message": message}),
            ))))
            .await;
    }

    if let Some(choice) = parsed.choices.into_iter().next() {
        if let Some(delta) = choice.delta.content {
            if !delta.is_empty() {
                state.output_text.push_str(&delta);
                let index = ensure_text_block(state, tx).await;
                let _ = tx
                    .send(Ok(Bytes::from(sse_event(
                        "content_block_delta",
                        json!({
                            "type":"content_block_delta",
                            "index": index,
                            "delta": {"type":"text_delta","text": delta}
                        }),
                    ))))
                    .await;
            }
        }

        if let Some(reasoning) = choice.delta.reasoning_content {
            if reasoning.is_object() {
                let parsed: Result<crate::models::OpenAIReasoningContentDelta, _> =
                    serde_json::from_value(reasoning);
                if let Ok(delta) = parsed {
                    let index = ensure_thinking_block(state, tx).await;
                    if let Some(thinking) = delta.thinking {
                        state.reasoning_text.push_str(&thinking);
                        let _ = tx
                            .send(Ok(Bytes::from(sse_event(
                                "content_block_delta",
                                json!({
                                    "type":"content_block_delta",
                                    "index": index,
                                    "delta": {"type":"thinking_delta","thinking": thinking}
                                }),
                            ))))
                            .await;
                    }
                    if let Some(signature) = delta.signature {
                        state.reasoning_signature = Some(signature.clone());
                        let _ = tx
                            .send(Ok(Bytes::from(sse_event(
                                "content_block_delta",
                                json!({
                                    "type":"content_block_delta",
                                    "index": index,
                                    "delta": {"type":"signature_delta","signature": signature}
                                }),
                            ))))
                            .await;
                    }
                }
            } else if let Some(thinking) = reasoning.as_str() {
                state.reasoning_text.push_str(thinking);
                let index = ensure_thinking_block(state, tx).await;
                let _ = tx
                    .send(Ok(Bytes::from(sse_event(
                        "content_block_delta",
                        json!({
                            "type":"content_block_delta",
                            "index": index,
                            "delta": {"type":"thinking_delta","thinking": thinking}
                        }),
                    ))))
                    .await;
            }
        }

        if let Some(tool_calls) = choice.delta.tool_calls {
            for call in tool_calls {
                let entry = state.tool_calls.entry(call.index).or_insert_with(|| {
                    let index = state.next_index;
                    state.next_index += 1;
                    ToolCallState {
                        id: None,
                        name: None,
                        arguments: String::new(),
                        block_index: index,
                        started: false,
                        stopped: false,
                    }
                });

                if let Some(id) = call.id {
                    entry.id = Some(id);
                }
                if let Some(call_type) = call.call_type {
                    let _ = call_type;
                }
                if let Some(function) = call.function {
                    if let Some(name) = function.name {
                        entry.name = Some(name);
                    }
                    if let Some(args) = function.arguments {
                        entry.arguments.push_str(&args);
                        if entry.started {
                            let _ = tx
                                .send(Ok(Bytes::from(sse_event(
                                    "content_block_delta",
                                    json!({
                                        "type":"content_block_delta",
                                        "index": entry.block_index,
                                        "delta": {"type":"input_json_delta","partial_json": args}
                                    }),
                                ))))
                                .await;
                        }
                    }
                }

                if !entry.started && entry.id.is_some() && entry.name.is_some() {
                    entry.started = true;
                    let _ = tx
                        .send(Ok(Bytes::from(sse_event(
                            "content_block_start",
                            json!({
                                "type":"content_block_start",
                                "index": entry.block_index,
                                "content_block": {
                                    "type":"tool_use",
                                    "id": entry.id,
                                    "name": entry.name,
                                    "input": {}
                                }
                            }),
                        ))))
                        .await;
                    if !entry.arguments.is_empty() {
                        let buffered = entry.arguments.clone();
                        let _ = tx
                            .send(Ok(Bytes::from(sse_event(
                                "content_block_delta",
                                json!({
                                    "type":"content_block_delta",
                                    "index": entry.block_index,
                                    "delta": {"type":"input_json_delta","partial_json": buffered}
                                }),
                            ))))
                            .await;
                    }
                }
            }
        }

        if let Some(finish) = choice.finish_reason {
            flush_open_blocks(state, tx).await?;
            let stop_reason = map_finish_reason(&finish);
            let _ = tx
                .send(Ok(Bytes::from(sse_event(
                    "message_delta",
                    json!({
                        "type":"message_delta",
                        "delta": {"stop_reason": stop_reason},
                        "usage": {"output_tokens": 0}
                    }),
                ))))
                .await;
        }
    }

    Ok(())
}

fn stream_output_messages(state: &StreamState) -> Option<serde_json::Value> {
    let mut msg = serde_json::Map::new();
    if !state.reasoning_text.is_empty() {
        msg.insert(
            "reasoning_content".to_string(),
            serde_json::Value::String(state.reasoning_text.clone()),
        );
    }
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    for tool in state.tool_calls.values() {
        if tool.name.is_none() {
            continue;
        }
        let id = tool
            .id
            .clone()
            .unwrap_or_else(|| "tool_call".to_string());
        let name = tool.name.clone().unwrap_or_default();
        tool_calls.push(serde_json::json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": tool.arguments,
            }
        }));
    }

    msg.insert(
        "role".to_string(),
        serde_json::Value::String("assistant".to_string()),
    );

    if !tool_calls.is_empty() {
        msg.insert("tool_calls".to_string(), serde_json::Value::Array(tool_calls));
    }

    if !state.output_text.is_empty() {
        msg.insert(
            "content".to_string(),
            serde_json::Value::String(state.output_text.clone()),
        );
    }

    if msg.len() == 1 {
        return None;
    }

    Some(serde_json::Value::Array(vec![serde_json::Value::Object(msg)]))
}

fn stream_upstream_response(state: &StreamState) -> Option<String> {
    let mut content: Vec<serde_json::Value> = Vec::new();

    if !state.reasoning_text.is_empty() || state.reasoning_signature.is_some() {
        content.push(serde_json::json!({
            "type": "thinking",
            "thinking": state.reasoning_text,
            "signature": state.reasoning_signature.clone().unwrap_or_else(|| "auto".to_string())
        }));
    }

    if !state.output_text.is_empty() {
        content.push(serde_json::json!({
            "type": "text",
            "text": state.output_text
        }));
    }

    for tool in state.tool_calls.values() {
        if tool.name.is_none() {
            continue;
        }
        let id = tool
            .id
            .clone()
            .unwrap_or_else(|| "tool_use".to_string());
        let name = tool.name.clone().unwrap_or_default();
        let input = serde_json::from_str::<serde_json::Value>(&tool.arguments)
            .unwrap_or_else(|_| serde_json::json!({}));
        content.push(serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input
        }));
    }

    if content.is_empty() {
        return None;
    }

    let message = serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": content,
        "stop_reason": "tool_use"
    });
    serde_json::to_string(&message).ok()
}

async fn ensure_text_block(state: &mut StreamState, tx: &mpsc::Sender<Result<Bytes, std::convert::Infallible>>) -> u32 {
    if let Some(index) = state.text_block_index {
        return index;
    }
    let index = state.next_index;
    state.next_index += 1;
    state.text_block_index = Some(index);
    let _ = tx
        .send(Ok(Bytes::from(sse_event(
            "content_block_start",
            json!({
                "type":"content_block_start",
                "index": index,
                "content_block": {"type":"text","text":""}
            }),
        ))))
        .await;
    index
}

async fn ensure_thinking_block(
    state: &mut StreamState,
    tx: &mpsc::Sender<Result<Bytes, std::convert::Infallible>>,
) -> u32 {
    if let Some(index) = state.thinking_block_index {
        return index;
    }
    let index = state.next_index;
    state.next_index += 1;
    state.thinking_block_index = Some(index);
    let _ = tx
        .send(Ok(Bytes::from(sse_event(
            "content_block_start",
            json!({
                "type":"content_block_start",
                "index": index,
                "content_block": {"type":"thinking","thinking":"","signature":""}
            }),
        ))))
        .await;
    index
}

async fn flush_open_blocks(
    state: &mut StreamState,
    tx: &mpsc::Sender<Result<Bytes, std::convert::Infallible>>,
) -> Result<(), AppError> {
    if let Some(index) = state.text_block_index.take() {
        let _ = tx
            .send(Ok(Bytes::from(sse_event(
                "content_block_stop",
                json!({"type":"content_block_stop","index": index}),
            ))))
            .await;
    }

    if let Some(index) = state.thinking_block_index.take() {
        let _ = tx
            .send(Ok(Bytes::from(sse_event(
                "content_block_stop",
                json!({"type":"content_block_stop","index": index}),
            ))))
            .await;
    }

    for tool in state.tool_calls.values_mut() {
        if tool.started {
            if tool.arguments.is_empty() {
                return Err(AppError::invalid_request("tool_use arguments empty"));
            }
            if serde_json::from_str::<serde_json::Value>(&tool.arguments).is_err() {
                return Err(AppError::invalid_request("tool_use arguments invalid json"));
            }
        }
        if !tool.stopped {
            let _ = tx
                .send(Ok(Bytes::from(sse_event(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index": tool.block_index}),
                ))))
                .await;
            tool.stopped = true;
        }
    }

    Ok(())
}

fn sse_event(event: &str, data: serde_json::Value) -> String {
    format!("event: {}\ndata: {}\n\n", event, data)
}

fn error_event(err: AppError) -> String {
    let body = json!({
        "type": "error",
        "error": {"type": err.error_type, "message": err.message}
    });
    sse_event("error", body)
}

fn usage_zero() -> AnthropicUsage {
    AnthropicUsage {
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    }
}

fn map_finish_reason(reason: &str) -> &str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        _ => "end_turn",
    }
}

fn append_trace(buf: &mut String, chunk: &str) {
    buf.push_str(chunk);
}

fn serialize_json_for_trace(value: &serde_json::Value) -> String {
    match serde_json::to_string(value) {
        Ok(s) => s,
        Err(_) => "[unserializable]".to_string(),
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_chunk_emits_message_and_text_delta() {
        let (tx, mut rx) = mpsc::channel::<Result<Bytes, std::convert::Infallible>>(8);
        let mut state = StreamState {
            started: false,
            message_id: None,
            model: None,
            next_index: 0,
            text_block_index: None,
            thinking_block_index: None,
            tool_calls: HashMap::new(),
            output_text: String::new(),
            reasoning_text: String::new(),
            reasoning_signature: None,
        };

        let chunk = OpenAIStreamChunk {
            id: Some("chatcmpl-test".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            choices: vec![crate::models::OpenAIStreamChoice {
                index: 0,
                delta: crate::models::OpenAIStreamDelta {
                    role: Some("assistant".to_string()),
                    content: Some("Hi".to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: None,
            }],
        };

        handle_openai_chunk(chunk, &mut state, &tx)
            .await
            .expect("ok");
        drop(tx);

        let mut output = String::new();
        while let Some(item) = rx.recv().await {
            if let Ok(bytes) = item {
                output.push_str(&String::from_utf8_lossy(&bytes));
            }
        }

        assert!(output.contains("message_start"));
        assert!(output.contains("text_delta"));
    }

    #[tokio::test]
    async fn stream_chunk_emits_tool_use_with_input_json() {
        let (tx, mut rx) = mpsc::channel::<Result<Bytes, std::convert::Infallible>>(16);
        let mut state = StreamState {
            started: false,
            message_id: None,
            model: None,
            next_index: 0,
            text_block_index: None,
            thinking_block_index: None,
            tool_calls: HashMap::new(),
            output_text: String::new(),
            reasoning_text: String::new(),
            reasoning_signature: None,
        };

        let chunk = OpenAIStreamChunk {
            id: Some("chatcmpl-tool".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            choices: vec![crate::models::OpenAIStreamChoice {
                index: 0,
                delta: crate::models::OpenAIStreamDelta {
                    role: Some("assistant".to_string()),
                    content: None,
                    tool_calls: Some(vec![crate::models::OpenAIToolCallDelta {
                        index: 0,
                        id: Some("call_1".to_string()),
                        call_type: Some("function".to_string()),
                        function: Some(crate::models::OpenAIToolCallFunctionDelta {
                            name: Some("get_weather".to_string()),
                            arguments: Some("{\"location\":\"北京\"}".to_string()),
                        }),
                    }]),
                    reasoning_content: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
        };

        handle_openai_chunk(chunk, &mut state, &tx)
            .await
            .expect("ok");
        drop(tx);

        let mut output = String::new();
        while let Some(item) = rx.recv().await {
            if let Ok(bytes) = item {
                output.push_str(&String::from_utf8_lossy(&bytes));
            }
        }

        assert!(output.contains("tool_use"));
        assert!(output.contains("input_json_delta"));
        assert!(output.contains("message_delta"));
    }

    #[tokio::test]
    async fn stream_invalid_tool_use_arguments_emits_error() {
        let (tx, mut rx) = mpsc::channel::<Result<Bytes, std::convert::Infallible>>(16);
        let mut state = StreamState {
            started: false,
            message_id: None,
            model: None,
            next_index: 0,
            text_block_index: None,
            thinking_block_index: None,
            tool_calls: HashMap::new(),
            output_text: String::new(),
            reasoning_text: String::new(),
            reasoning_signature: None,
        };

        let chunk = OpenAIStreamChunk {
            id: Some("chatcmpl-tool".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            choices: vec![crate::models::OpenAIStreamChoice {
                index: 0,
                delta: crate::models::OpenAIStreamDelta {
                    role: Some("assistant".to_string()),
                    content: None,
                    tool_calls: Some(vec![crate::models::OpenAIToolCallDelta {
                        index: 0,
                        id: Some("call_1".to_string()),
                        call_type: Some("function".to_string()),
                        function: Some(crate::models::OpenAIToolCallFunctionDelta {
                            name: Some("get_weather".to_string()),
                            arguments: Some("{\"location\":".to_string()),
                        }),
                    }]),
                    reasoning_content: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
        };

        let err = handle_openai_chunk(chunk, &mut state, &tx)
            .await
            .expect_err("should fail");
        let _ = tx
            .send(Ok(Bytes::from(error_event(err))))
            .await;

        drop(tx);

        let mut output = String::new();
        while let Some(item) = rx.recv().await {
            if let Ok(bytes) = item {
                output.push_str(&String::from_utf8_lossy(&bytes));
            }
        }

        assert!(output.contains("invalid_request_error"));
        assert!(!output.contains("message_delta"));
    }

    #[test]
    fn stream_output_messages_includes_tool_calls() {
        let mut state = StreamState {
            started: true,
            message_id: Some("chatcmpl-test".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            next_index: 1,
            text_block_index: None,
            thinking_block_index: None,
            tool_calls: HashMap::from([(
                0,
                ToolCallState {
                    id: Some("call_1".to_string()),
                    name: Some("get_weather".to_string()),
                    arguments: "{\"location\":\"Beijing\"}".to_string(),
                    block_index: 0,
                    started: true,
                    stopped: true,
                },
            )]),
            output_text: String::new(),
            reasoning_text: String::new(),
            reasoning_signature: None,
        };

        let output = stream_output_messages(&state).expect("output");
        let value = output.as_array().expect("array");
        assert_eq!(value.len(), 1);
        let value = value[0].as_object().expect("object");
        assert!(value.get("tool_calls").is_some());
    }
}
