use crate::config::{Config, DocumentPolicy};
use crate::models::*;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct TranslateError {
    pub error_type: String,
    pub message: String,
}

impl TranslateError {
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            error_type: "invalid_request_error".to_string(),
            message: message.into(),
        }
    }

    pub fn api_error(message: impl Into<String>) -> Self {
        Self {
            error_type: "api_error".to_string(),
            message: message.into(),
        }
    }
}

pub fn anthropic_to_openai(req: AnthropicRequest, config: &Config) -> Result<OpenAIRequest, TranslateError> {
    let mut messages = Vec::new();
    let reasoning_effort = req
        .thinking
        .as_ref()
        .and_then(|thinking| map_reasoning_effort(thinking, config));
    let include_reasoning = reasoning_effort.is_some();

    if let Some(system) = req.system {
        let system_text = extract_system_text(system)?;
        if !system_text.is_empty() {
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(OpenAIMessageContent::Text(system_text)),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            });
        }
    }

    for msg in req.messages {
        if msg.role != "user" && msg.role != "assistant" {
            return Err(TranslateError::invalid_request(format!(
                "messages: Unexpected role \"{}\"",
                msg.role
            )));
        }
        let converted = convert_message(msg.role, msg.content, config, include_reasoning)?;
        messages.extend(converted);
    }

    let tools = req.tools.map(anthropic_tools_to_openai_tools);
    let tool_choice = req.tool_choice.map(anthropic_tool_choice_to_openai);
    let response_format = req
        .output_format
        .map(|format| anthropic_output_format_to_openai(format, config.models.output_strict));
    Ok(OpenAIRequest {
        model: req.model,
        messages,
        max_completion_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        stop: req.stop_sequences,
        stream: req.stream,
        tools,
        tool_choice,
        response_format,
        reasoning_effort,
        stream_options: req.stream.map(|stream| OpenAIStreamOptions {
            include_usage: stream,
        }),
    })
}

pub fn openai_to_anthropic(resp: OpenAIResponse) -> Result<AnthropicResponse, TranslateError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| TranslateError::api_error("missing choices in response"))?;

    let mut content_blocks: Vec<AnthropicContentBlock> = Vec::new();

    if let Some(reasoning) = choice.message.reasoning_content {
        if reasoning.is_object() {
            let parsed: Result<OpenAIReasoningContent, _> = serde_json::from_value(reasoning);
            if let Ok(reasoning) = parsed {
                content_blocks.push(AnthropicContentBlock::Thinking {
                    thinking: reasoning.thinking,
                    signature: reasoning.signature,
                });
            }
        } else if let Some(thinking) = reasoning.as_str() {
            content_blocks.push(AnthropicContentBlock::Thinking {
                thinking: thinking.to_string(),
                signature: "auto".to_string(),
            });
        }
    }

    if let Some(tool_calls) = choice.message.tool_calls {
        for call in tool_calls {
            let input: Value = serde_json::from_str(&call.function.arguments).map_err(|e| {
                TranslateError::api_error(format!("invalid tool call arguments: {}", e))
            })?;
            content_blocks.push(AnthropicContentBlock::ToolUse {
                id: call.id,
                name: call.function.name,
                input,
            });
        }
    }

    if let Some(content) = choice.message.content {
        content_blocks.push(AnthropicContentBlock::Text {
            text: content,
            cache_control: None,
        });
    }

    if content_blocks.is_empty() {
        return Err(TranslateError::api_error("missing assistant content"));
    }

    let stop_reason = match choice.finish_reason.as_deref() {
        Some("stop") | None => "end_turn",
        Some("length") => "max_tokens",
        Some("tool_calls") => "tool_use",
        _ => "end_turn",
    }
    .to_string();

    let usage = match resp.usage {
        Some(u) => AnthropicUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
        None => AnthropicUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
    };

    Ok(AnthropicResponse {
        id: resp.id,
        response_type: "message".to_string(),
        role: "assistant".to_string(),
        model: resp.model,
        content: content_blocks,
        stop_reason,
        stop_sequence: None,
        usage,
    })
}

pub fn openai_models_to_anthropic(
    resp: OpenAIModelsResponse,
    model_display_map: &std::collections::HashMap<String, String>,
) -> Result<AnthropicModelsResponse, TranslateError> {
    let mut data = Vec::new();
    for model in resp.data {
        let display_name = model_display_map
            .get(&model.id)
            .cloned()
            .unwrap_or_else(|| titleize_model_id(&model.id));
        let created_at = match model.created {
            Some(ts) => unix_to_iso8601(ts)?,
            None => "1970-01-01T00:00:00Z".to_string(),
        };
        data.push(AnthropicModel {
            id: model.id,
            model_type: "model".to_string(),
            display_name,
            created_at,
        });
    }
    Ok(AnthropicModelsResponse { data })
}

fn unix_to_iso8601(ts: i64) -> Result<String, TranslateError> {
    if ts < 0 {
        return Err(TranslateError::invalid_request("invalid created timestamp"));
    }
    let secs = ts as u64;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let rem = rem % 3_600;
    let min = rem / 60;
    let sec = rem % 60;

    let (year, month, day) = civil_from_days(days as i64);
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    ))
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (year, m as u32, d as u32)
}

fn titleize_model_id(id: &str) -> String {
    let mut out = String::new();
    for (i, part) in id.split('-').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if part.len() <= 3 && part.chars().all(|c| c.is_ascii_alphanumeric()) {
            out.push_str(&part.to_uppercase());
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            for c in chars {
                out.push(c.to_ascii_lowercase());
            }
        }
    }
    out
}

fn convert_message(
    role: String,
    content: AnthropicContent,
    config: &Config,
    include_reasoning: bool,
) -> Result<Vec<OpenAIMessage>, TranslateError> {
    match content {
        AnthropicContent::Text(s) => {
            let reasoning_content = if include_reasoning && role == "assistant" {
                Some(Value::String(String::new()))
            } else {
                None
            };
            Ok(vec![OpenAIMessage {
                role,
                content: Some(OpenAIMessageContent::Text(s)),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content,
            }])
        }
        AnthropicContent::Blocks(blocks) => {
            let document_policy = config
                .document_policy()
                .map_err(TranslateError::invalid_request)?;
            let mut messages: Vec<OpenAIMessage> = Vec::new();
            let mut parts: Vec<OpenAIContentPart> = Vec::new();
            let mut thinking_text: Option<String> = None;

            let mut flush_parts = |messages: &mut Vec<OpenAIMessage>, parts: &mut Vec<OpenAIContentPart>, thinking_text: &Option<String>| {
                if parts.is_empty() {
                    return;
                }
                let content = if parts.len() == 1 {
                    match parts.remove(0) {
                        OpenAIContentPart::Text { text } => OpenAIMessageContent::Text(text),
                        part => OpenAIMessageContent::Parts(vec![part]),
                    }
                } else {
                    OpenAIMessageContent::Parts(std::mem::take(parts))
                };

                let reasoning_content = if include_reasoning && role == "assistant" {
                    Some(Value::String(thinking_text.clone().unwrap_or_default()))
                } else {
                    None
                };
                messages.push(OpenAIMessage {
                    role: role.clone(),
                    content: Some(content),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content,
                });
            };

            for block in blocks {
                match block {
                    AnthropicContentBlock::Text { text, .. } => {
                        parts.push(OpenAIContentPart::Text { text });
                    }
                    AnthropicContentBlock::Image { source } => {
                        if !config.models.allow_images {
                            return Err(TranslateError::invalid_request(
                                "image content not allowed",
                            ));
                        }
                        let media_type = source
                            .media_type
                            .ok_or_else(|| TranslateError::invalid_request("image media_type missing"))?;
                        let data = source
                            .data
                            .ok_or_else(|| TranslateError::invalid_request("image data missing"))?;
                        let url = format!("data:{};base64,{}", media_type, data);
                        parts.push(OpenAIContentPart::ImageUrl {
                            image_url: OpenAIImageUrl { url, detail: None },
                        });
                    }
                    AnthropicContentBlock::Document { .. } => match document_policy {
                        DocumentPolicy::Reject => {
                            return Err(TranslateError::invalid_request(
                                "document content not supported",
                            ));
                        }
                        DocumentPolicy::Strip => {
                            continue;
                        }
                        DocumentPolicy::TextOnly => {
                            parts.push(OpenAIContentPart::Text {
                                text: "[document omitted]".to_string(),
                            });
                        }
                    },
                    AnthropicContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        flush_parts(&mut messages, &mut parts, &thinking_text);
                        let text = match content {
                            Value::String(s) => s,
                            other => serde_json::to_string(&other).map_err(|e| {
                                TranslateError::invalid_request(format!(
                                    "tool_result content invalid: {}",
                                    e
                                ))
                            })?,
                        };
                        messages.push(OpenAIMessage {
                            role: "tool".to_string(),
                            content: Some(OpenAIMessageContent::Text(text)),
                            tool_calls: None,
                            tool_call_id: Some(tool_use_id),
                            reasoning_content: None,
                        });
                    }
                    AnthropicContentBlock::ToolUse { id, name, input } => {
                        flush_parts(&mut messages, &mut parts, &thinking_text);
                        if role != "assistant" {
                            return Err(TranslateError::invalid_request(
                                "tool_use must be in assistant role",
                            ));
                        }
                        let arguments = serde_json::to_string(&input).map_err(|e| {
                            TranslateError::invalid_request(format!(
                                "tool_use input invalid: {}",
                                e
                            ))
                        })?;
                        let reasoning_content =
                            Some(Value::String(thinking_text.clone().unwrap_or_default()));
                        if let Some(last) = messages.last_mut() {
                            if last.role == "assistant" && last.tool_calls.is_some() && last.content.is_none() {
                                if let Some(tool_calls) = last.tool_calls.as_mut() {
                                    tool_calls.push(OpenAIToolCall {
                                        id,
                                        call_type: "function".to_string(),
                                        function: OpenAIToolCallFunction { name, arguments },
                                    });
                                    if last.reasoning_content.is_none() {
                                        last.reasoning_content = reasoning_content;
                                    }
                                    continue;
                                }
                            }
                        }
                        messages.push(OpenAIMessage {
                            role: "assistant".to_string(),
                            content: None,
                            tool_calls: Some(vec![OpenAIToolCall {
                                id,
                                call_type: "function".to_string(),
                                function: OpenAIToolCallFunction { name, arguments },
                            }]),
                            tool_call_id: None,
                            reasoning_content,
                        });
                    }
                    AnthropicContentBlock::Thinking { thinking, .. } => {
                        thinking_text = Some(thinking);
                        continue;
                    }
                    AnthropicContentBlock::RedactedThinking { .. } => {
                        thinking_text = Some(String::new());
                        continue;
                    }
                }
            }

            flush_parts(&mut messages, &mut parts, &thinking_text);
            Ok(messages)
        }
    }
}

fn extract_system_text(system: AnthropicSystem) -> Result<String, TranslateError> {
    match system {
        AnthropicSystem::Text(s) => Ok(s),
        AnthropicSystem::Blocks(blocks) => {
            let mut out = String::new();
            for block in blocks {
                if block.block_type != "text" {
                    return Err(TranslateError::invalid_request(format!(
                        "system block type not supported: {}",
                        block.block_type
                    )));
                }
                let text = block.text.unwrap_or_default();
                out.push_str(&text);
            }
            Ok(out)
        }
    }
}

fn anthropic_tools_to_openai_tools(tools: Vec<AnthropicTool>) -> Vec<OpenAITool> {
    tools
        .into_iter()
        .map(|tool| OpenAITool {
            tool_type: "function".to_string(),
            function: OpenAIFunctionDef {
                name: tool.name,
                description: tool.description,
                parameters: tool.input_schema,
            },
        })
        .collect()
}

fn anthropic_tool_choice_to_openai(choice: AnthropicToolChoice) -> OpenAIToolChoice {
    match choice.choice_type.as_str() {
        "auto" => OpenAIToolChoice::Mode("auto".to_string()),
        "any" => OpenAIToolChoice::Mode("auto".to_string()),
        "tool" => {
            let name = choice.name.unwrap_or_default();
            OpenAIToolChoice::Tool(OpenAIToolChoiceFunction {
                choice_type: "function".to_string(),
                function: OpenAIToolChoiceName { name },
            })
        }
        other => OpenAIToolChoice::Mode(other.to_string()),
    }
}

fn anthropic_output_format_to_openai(
    format: AnthropicOutputFormat,
    output_strict: bool,
) -> OpenAIResponseFormat {
    let json_schema = OpenAIJsonSchema {
        name: None,
        schema: format.schema,
        strict: Some(output_strict),
    };

    OpenAIResponseFormat {
        format_type: "json_schema".to_string(),
        json_schema: Some(json_schema),
    }
}

fn map_reasoning_effort(thinking: &AnthropicThinking, config: &Config) -> Option<String> {
    let budget = thinking.budget_tokens?;
    for (threshold, effort) in config.thinking_map_pairs().iter().rev() {
        if budget >= *threshold {
            return Some(effort.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            server: crate::config::ServerConfig {
                bind_addr: "127.0.0.1:0".to_string(),
            },
            downstream: crate::config::DownstreamConfig {
                base_url: "https://api.openai.com".to_string(),
                api_key: Some("sk-test".to_string()),
                anthropic_version: Some("2023-06-01".to_string()),
                anthropic_beta: None,
                connect_timeout_ms: 5000,
                read_timeout_ms: 30000,
                pool_max_idle_per_host: 64,
            },
            anthropic: crate::config::AnthropicConfig {
                forward_mode: "passthrough".to_string(),
            },
            models: crate::config::ModelsConfig {
                model_map: Default::default(),
                display_map: Default::default(),
                allowlist: Default::default(),
                blocklist: Default::default(),
                thinking_map: std::collections::HashMap::from([
                    (4000, "medium".to_string()),
                    (8000, "high".to_string()),
                ]),
                output_strict: true,
                allow_images: true,
                document_policy: "reject".to_string(),
                models_override: None,
            },
            limits: crate::config::LimitsConfig { max_inflight: 64 },
            observability: crate::config::ObservabilityConfig {
                service_name: "llm-gateway".to_string(),
                dump_downstream: false,
                audit_log: crate::config::AuditLogConfig::default(),
                logging: crate::config::LoggingConfig {
                    level: "info".to_string(),
                    format: "text".to_string(),
                    stdout: false,
                    file: None,
                },
                otlp_grpc: crate::config::OtlpGrpcConfig {
                    endpoint: "http://localhost:4317".to_string(),
                    timeout_ms: 3000,
                },
                otlp_http: crate::config::OtlpHttpConfig {
                    base_url: "https://cloud.langfuse.com/api/public/otel".to_string(),
                    public_key: "".to_string(),
                    secret_key: "".to_string(),
                    timeout_ms: 5000,
                },
                exporters: crate::config::ExportersConfig {
                    tracing: "otlp_grpc".to_string(),
                    metrics: "otlp_grpc".to_string(),
                },
            },
        }
    }

    #[test]
    fn anthropic_to_openai_text_only() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 64,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Text("Hello".to_string()),
            }],
            system: Some(AnthropicSystem::Text("You are helpful".to_string())),
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: None,
            stop_sequences: Some(vec!["STOP".to_string()]),
            stream: Some(false),
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let out = anthropic_to_openai(req, &base_config()).expect("translate ok");
        assert_eq!(out.model, "gpt-4o-mini");
        assert_eq!(out.max_completion_tokens, 64);
        assert_eq!(out.messages.len(), 2);
        assert_eq!(out.messages[0].role, "system");
        assert_eq!(out.messages[1].role, "user");
        assert_eq!(out.temperature, Some(0.7));
        assert_eq!(out.top_p, Some(0.9));
        assert_eq!(out.stop, Some(vec!["STOP".to_string()]));
        assert_eq!(out.stream, Some(false));
    }

    #[test]
    fn anthropic_to_openai_rejects_non_text_block() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 64,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Blocks(vec![AnthropicContentBlock::Document {
                    source: AnthropicSource {
                        source_type: "base64".to_string(),
                        media_type: Some("application/pdf".to_string()),
                        data: Some("AAA".to_string()),
                        cache_control: None,
                    },
                }]),
            }],
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(false),
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let err = anthropic_to_openai(req, &base_config()).expect_err("should reject");
        assert_eq!(err.error_type, "invalid_request_error");
    }

    #[test]
    fn openai_to_anthropic_text_response() {
        let resp = OpenAIResponse {
            id: "chatcmpl-123".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: Some("Hi".to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OpenAIUsage {
                prompt_tokens: 5,
                completion_tokens: 7,
                total_tokens: 12,
            }),
        };

        let out = openai_to_anthropic(resp).expect("translate ok");
        assert_eq!(out.id, "chatcmpl-123");
        assert_eq!(out.model, "gpt-4o-mini");
        assert_eq!(out.role, "assistant");
        assert_eq!(out.stop_reason, "end_turn");
        assert_eq!(out.content.len(), 1);
        match &out.content[0] {
            AnthropicContentBlock::Text { text, .. } => assert_eq!(text, "Hi"),
            _ => panic!("unexpected block"),
        }
        assert_eq!(out.usage.input_tokens, 5);
        assert_eq!(out.usage.output_tokens, 7);
    }

    #[test]
    fn anthropic_system_blocks_concat() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 8,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Text("Ping".to_string()),
            }],
            system: Some(AnthropicSystem::Blocks(vec![
                AnthropicSystemBlock {
                    block_type: "text".to_string(),
                    text: Some("A".to_string()),
                },
                AnthropicSystemBlock {
                    block_type: "text".to_string(),
                    text: Some("B".to_string()),
                },
            ])),
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(false),
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let out = anthropic_to_openai(req, &base_config()).expect("translate ok");
        assert_eq!(out.messages.len(), 2);
        assert_eq!(out.messages[0].role, "system");
        match &out.messages[0].content {
            Some(OpenAIMessageContent::Text(text)) => assert_eq!(text, "AB"),
            _ => panic!("unexpected system content"),
        }
    }

    #[test]
    fn openai_to_anthropic_finish_reason_mappings() {
        let resp = OpenAIResponse {
            id: "chatcmpl-456".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: Some("Hi".to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: Some("length".to_string()),
            }],
            usage: None,
        };

        let out = openai_to_anthropic(resp).expect("translate ok");
        assert_eq!(out.stop_reason, "max_tokens");

        let resp_tool = OpenAIResponse {
            id: "chatcmpl-789".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: Some("".to_string()),
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: None,
        };

        let out_tool = openai_to_anthropic(resp_tool).expect("translate ok");
        assert_eq!(out_tool.stop_reason, "tool_use");
    }

    #[test]
    fn openai_to_anthropic_missing_choices() {
        let resp = OpenAIResponse {
            id: "chatcmpl-empty".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![],
            usage: None,
        };

        let err = openai_to_anthropic(resp).expect_err("should fail");
        assert_eq!(err.error_type, "api_error");
    }

    #[test]
    fn openai_to_anthropic_missing_content() {
        let resp = OpenAIResponse {
            id: "chatcmpl-missing".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: None,
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };

        let err = openai_to_anthropic(resp).expect_err("should fail");
        assert_eq!(err.error_type, "api_error");
    }

    #[test]
    fn anthropic_system_blocks_rejects_non_text() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 8,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Text("Ping".to_string()),
            }],
            system: Some(AnthropicSystem::Blocks(vec![AnthropicSystemBlock {
                block_type: "image".to_string(),
                text: None,
            }])),
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(false),
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let err = anthropic_to_openai(req, &base_config()).expect_err("should reject");
        assert_eq!(err.error_type, "invalid_request_error");
    }

    #[test]
    fn anthropic_to_openai_allows_streaming() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 8,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Text("Ping".to_string()),
            }],
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(true),
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let out = anthropic_to_openai(req, &base_config()).expect("translate ok");
        assert_eq!(out.stream, Some(true));
    }

    #[test]
    fn anthropic_to_openai_rejects_tool_use_role() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 8,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Blocks(vec![AnthropicContentBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "get_weather".to_string(),
                    input: serde_json::json!({"location":"beijing"}),
                }]),
            }],
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(false),
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let err = anthropic_to_openai(req, &base_config()).expect_err("should reject");
        assert_eq!(err.error_type, "invalid_request_error");
    }

    #[test]
    fn anthropic_tools_and_choice_mapping() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 16,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Text("Ping".to_string()),
            }],
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(false),
            tools: Some(vec![AnthropicTool {
                name: "get_weather".to_string(),
                description: Some("Get weather".to_string()),
                input_schema: serde_json::json!({"type":"object","properties":{"location":{"type":"string"}}}),
            }]),
            tool_choice: Some(AnthropicToolChoice {
                choice_type: "tool".to_string(),
                name: Some("get_weather".to_string()),
            }),
            output_format: None,
            thinking: None,
        };

        let out = anthropic_to_openai(req, &base_config()).expect("translate ok");
        let tools = out.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "get_weather");
        match out.tool_choice.expect("tool_choice") {
            OpenAIToolChoice::Tool(choice) => assert_eq!(choice.function.name, "get_weather"),
            _ => panic!("unexpected tool choice"),
        }
    }

    #[test]
    fn anthropic_output_format_mapping() {
        let req = AnthropicRequest {
            model: "gpt-4o-mini".to_string(),
            max_tokens: 16,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Text("Ping".to_string()),
            }],
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: Some(false),
            tools: None,
            tool_choice: None,
            output_format: Some(AnthropicOutputFormat {
                format_type: "json".to_string(),
                schema: serde_json::json!({"type":"object"}),
            }),
            thinking: None,
        };

        let out = anthropic_to_openai(req, &base_config()).expect("translate ok");
        let response_format = out.response_format.expect("response_format");
        assert_eq!(response_format.format_type, "json_schema");
        assert_eq!(
            response_format.json_schema.unwrap().schema,
            serde_json::json!({"type":"object"})
        );
    }

    #[test]
    fn openai_tool_calls_to_anthropic_tool_use() {
        let resp = OpenAIResponse {
            id: "chatcmpl-tool".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: OpenAIToolCallFunction {
                            name: "get_weather".to_string(),
                            arguments: "{\"location\":\"Beijing\"}".to_string(),
                        },
                    }]),
                    reasoning_content: None,
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: None,
        };

        let out = openai_to_anthropic(resp).expect("translate ok");
        assert_eq!(out.stop_reason, "tool_use");
        match &out.content[0] {
            AnthropicContentBlock::ToolUse { name, .. } => assert_eq!(name, "get_weather"),
            _ => panic!("expected tool_use block"),
        }
    }

    #[test]
    fn anthropic_tool_uses_aggregate_into_single_openai_message() {
        let req = AnthropicRequest {
            model: "claude-3-5-sonnet".to_string(),
            max_tokens: 10,
            system: None,
            messages: vec![AnthropicMessage {
                role: "assistant".to_string(),
                content: AnthropicContent::Blocks(vec![
                    AnthropicContentBlock::ToolUse {
                        id: "tool_1".to_string(),
                        name: "get_weather".to_string(),
                        input: serde_json::json!({"location":"Beijing"}),
                    },
                    AnthropicContentBlock::ToolUse {
                        id: "tool_2".to_string(),
                        name: "get_time".to_string(),
                        input: serde_json::json!({"tz":"Asia/Shanghai"}),
                    },
                ]),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            output_format: None,
            thinking: None,
        };

        let out = anthropic_to_openai(req, &base_config()).expect("ok");
        assert_eq!(out.messages.len(), 1);
        let msg = &out.messages[0];
        assert_eq!(msg.role, "assistant");
        let tool_calls = msg.tool_calls.as_ref().expect("tool_calls");
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(tool_calls[1].function.name, "get_time");
    }

    #[test]
    fn openai_reasoning_to_anthropic_thinking() {
        let resp = OpenAIResponse {
            id: "chatcmpl-think".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: Some("Hi".to_string()),
                    tool_calls: None,
                    reasoning_content: Some(serde_json::json!({
                        "type": "thinking",
                        "thinking": "Step",
                        "signature": "sig"
                    })),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };

        let out = openai_to_anthropic(resp).expect("translate ok");
        match &out.content[0] {
            AnthropicContentBlock::Thinking { thinking, .. } => assert_eq!(thinking, "Step"),
            _ => panic!("expected thinking block"),
        }
    }

    #[test]
    fn openai_reasoning_string_to_anthropic_thinking() {
        let resp = OpenAIResponse {
            id: "chatcmpl-think-str".to_string(),
            model: "gpt-4o-mini".to_string(),
            choices: vec![OpenAIChoice {
                message: OpenAIChoiceMessage {
                    role: "assistant".to_string(),
                    content: Some("Hi".to_string()),
                    tool_calls: None,
                    reasoning_content: Some(serde_json::Value::String("Trace".to_string())),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };

        let out = openai_to_anthropic(resp).expect("translate ok");
        assert_eq!(out.content.len(), 2);
        match &out.content[0] {
            AnthropicContentBlock::Thinking { thinking, signature } => {
                assert_eq!(thinking, "Trace");
                assert_eq!(signature, "auto");
            }
            _ => panic!("expected thinking block"),
        }
    }

    #[test]
    fn openai_models_to_anthropic_mapping() {
        let resp = OpenAIModelsResponse {
            data: vec![OpenAIModel {
                id: "gpt-4o-mini".to_string(),
                object: Some("model".to_string()),
                created: Some(1_700_000_000),
                owned_by: Some("openai".to_string()),
            }],
        };
        let map = std::collections::HashMap::from([(
            "gpt-4o-mini".to_string(),
            "GPT-4o Mini".to_string(),
        )]);
        let out = openai_models_to_anthropic(resp, &map).expect("ok");
        assert_eq!(out.data.len(), 1);
        assert_eq!(out.data[0].id, "gpt-4o-mini");
        assert_eq!(out.data[0].model_type, "model");
        assert_eq!(out.data[0].display_name, "GPT-4o Mini");
        assert!(out.data[0].created_at.ends_with('Z'));
    }
}
