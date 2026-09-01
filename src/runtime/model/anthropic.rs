//! Anthropic Messages 原生 adapter。

use color_eyre::eyre::{Context, Result, bail};
use serde_json::Value;

use super::{
    AdapterStreamRequest, AdapterTitleRequest, BoxFuture, Completion, ModelCatalog, ModelInfo,
    ModelStreamEvent, ProviderAdapter, SseEvent, StopReason, StreamSink, Usage, apply_extra_body,
    diagnostic_label, object_mut, transport, u32_token,
};
use crate::{
    config::{
        CapabilitySupport, ModelCapabilities, ModelParameters, Provider, ProxySettings,
        ReasoningSettings,
    },
    session::{Message, Role},
};

pub(crate) static ANTHROPIC_ADAPTER: AnthropicAdapter = AnthropicAdapter;

pub(crate) struct AnthropicAdapter;

impl ProviderAdapter for AnthropicAdapter {
    fn stream_reply<'a>(
        &'a self,
        request: AdapterStreamRequest<'a>,
        sink: StreamSink,
    ) -> BoxFuture<'a, Completion> {
        Box::pin(async move { stream_messages(request, sink).await })
    }

    fn generate_title<'a>(&'a self, request: AdapterTitleRequest<'a>) -> BoxFuture<'a, String> {
        Box::pin(async move { generate_title(request).await })
    }

    fn fetch_models<'a>(
        &'a self,
        provider: &'a Provider,
        proxy: &'a ProxySettings,
    ) -> BoxFuture<'a, ModelCatalog> {
        Box::pin(async move { fetch_models(provider, proxy).await })
    }
}

async fn stream_messages(
    request: AdapterStreamRequest<'_>,
    sink: StreamSink,
) -> Result<Completion> {
    let transport = transport::HttpTransport::with_cancellation(
        request.provider,
        request.proxy,
        &request.settings.parameters,
        request.cancellation.clone(),
    )?;
    let mut messages = anthropic_messages(request.history);
    if let Some(compacted_context) = request.compacted_context {
        messages.insert(
            0,
            serde_json::json!({ "role": "user", "content": compacted_context }),
        );
    }
    let max_tokens = request
        .settings
        .parameters
        .max_output_tokens
        .or(request.settings.capabilities.max_output_tokens)
        .unwrap_or(4096);
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
    });
    apply_parameters(&mut body, &request.settings.parameters, max_tokens)?;
    apply_extra_body(&mut body, &request.settings.parameters.extra_body);
    let http_request = transport
        .client()
        .post(transport.endpoint("messages")?)
        .json(&body)
        .build()
        .context("failed to build Anthropic Messages request")?;
    let response = transport.send(http_request, "Anthropic Messages").await?;
    sink.record_request_id(transport::response_request_id(response.headers()));
    let mut state = AnthropicStreamState::default();
    transport::consume_sse(
        response,
        transport.policy.idle_timeout,
        request.cancellation,
        |event| state.handle(event, |event| sink.emit(event)),
    )
    .await?;
    state.finish()
}

fn anthropic_messages(history: &[Message]) -> Vec<Value> {
    history
        .iter()
        .filter(|message| message.role == Role::User || !message.content.is_empty())
        .map(|message| {
            serde_json::json!({
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": [{"type": "text", "text": message.content}],
            })
        })
        .collect()
}

fn apply_parameters(body: &mut Value, parameters: &ModelParameters, max_tokens: u32) -> Result<()> {
    let object = object_mut(body)?;
    if let Some(temperature) = parameters.temperature {
        object.insert("temperature".to_owned(), Value::from(temperature));
    }
    if let Some(top_p) = parameters.top_p {
        object.insert("top_p".to_owned(), Value::from(top_p));
    }
    if let Some(ReasoningSettings::Anthropic { budget_tokens }) = &parameters.reasoning {
        if *budget_tokens >= max_tokens {
            bail!("Anthropic thinking budget_tokens must be lower than max_tokens");
        }
        object.insert(
            "thinking".to_owned(),
            serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget_tokens,
            }),
        );
    }
    Ok(())
}

#[derive(Default)]
struct AnthropicStreamState {
    input_tokens: u64,
    cache_tokens: u64,
    output_tokens: u64,
    stop_reason: Option<StopReason>,
    completed: bool,
}

impl AnthropicStreamState {
    fn handle<F>(&mut self, event: SseEvent, mut emit: F) -> Result<()>
    where
        F: FnMut(ModelStreamEvent),
    {
        let payload: Value =
            serde_json::from_str(&event.data).context("invalid Anthropic Messages stream JSON")?;
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .ok_or_else(|| color_eyre::eyre::eyre!("Anthropic SSE event is missing type"))?;
        if let Some(name) = event.event.as_deref()
            && name != "message"
            && name != event_type
        {
            bail!(
                "Anthropic SSE event name {:?} does not match type {:?}",
                diagnostic_label(name),
                diagnostic_label(event_type)
            );
        }

        match event_type {
            "ping" => {}
            "message_start" => {
                let message = payload
                    .get("message")
                    .ok_or_else(|| color_eyre::eyre::eyre!("message_start is missing message"))?;
                let usage = message
                    .get("usage")
                    .ok_or_else(|| color_eyre::eyre::eyre!("message_start is missing usage"))?;
                self.input_tokens = usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        color_eyre::eyre::eyre!("message_start usage is missing input_tokens")
                    })?;
                self.cache_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
                    .saturating_add(
                        usage
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or_default(),
                    );
                self.output_tokens = usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
            }
            "content_block_start" => {
                let _index = payload
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        color_eyre::eyre::eyre!("content_block_start is missing index")
                    })?;
                let block = payload.get("content_block").ok_or_else(|| {
                    color_eyre::eyre::eyre!("content_block_start is missing content_block")
                })?;
                match block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text" => {
                        let text = block.get("text").and_then(Value::as_str).ok_or_else(|| {
                            color_eyre::eyre::eyre!("text content block is missing text")
                        })?;
                        if !text.is_empty() {
                            emit(ModelStreamEvent::AssistantTextDelta {
                                text: text.to_owned(),
                            });
                        }
                    }
                    "thinking" => {
                        let text =
                            block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    color_eyre::eyre::eyre!(
                                        "thinking content block is missing thinking"
                                    )
                                })?;
                        if !text.is_empty() {
                            emit(ModelStreamEvent::ReasoningDelta {
                                text: text.to_owned(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {}
            "content_block_delta" => {
                let delta = payload.get("delta").ok_or_else(|| {
                    color_eyre::eyre::eyre!("content_block_delta is missing delta")
                })?;
                let delta_type = delta.get("type").and_then(Value::as_str).ok_or_else(|| {
                    color_eyre::eyre::eyre!("content_block_delta is missing delta.type")
                })?;
                match delta_type {
                    "text_delta" => emit(ModelStreamEvent::AssistantTextDelta {
                        text: delta
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(|| color_eyre::eyre::eyre!("text_delta is missing text"))?
                            .to_owned(),
                    }),
                    "thinking_delta" => emit(ModelStreamEvent::ReasoningDelta {
                        text: delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                color_eyre::eyre::eyre!("thinking_delta is missing thinking")
                            })?
                            .to_owned(),
                    }),
                    "signature_delta" => {}
                    unknown => {
                        tracing::debug!(
                            event_type = "content_block_delta",
                            delta_type = %diagnostic_label(unknown),
                            "跳过 Anthropic 未来未知的 content delta"
                        );
                    }
                }
            }
            "message_delta" => {
                let delta = payload
                    .get("delta")
                    .ok_or_else(|| color_eyre::eyre::eyre!("message_delta is missing delta"))?;
                let stop_reason = delta
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        color_eyre::eyre::eyre!("message_delta is missing stop_reason")
                    })?;
                self.stop_reason = Some(match stop_reason {
                    "end_turn" => StopReason::Stop,
                    "stop_sequence" => StopReason::StopSequence,
                    "max_tokens" | "model_context_window_exceeded" => StopReason::MaxOutputTokens,
                    "tool_use" => StopReason::Other("tool_use".to_owned()),
                    "refusal" => StopReason::Refusal,
                    other => StopReason::Other(diagnostic_label(other)),
                });
                let usage = payload
                    .get("usage")
                    .ok_or_else(|| color_eyre::eyre::eyre!("message_delta is missing usage"))?;
                self.output_tokens = usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        color_eyre::eyre::eyre!("message_delta usage is missing output_tokens")
                    })?;
            }
            "message_stop" => self.completed = true,
            "error" => {
                let error_type = payload
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_error");
                bail!("Anthropic stream error type={error_type}");
            }
            unknown => {
                // Anthropic 官方版本策略要求客户端对未来事件类型保持可扩展。
                tracing::debug!(
                    event_type = %diagnostic_label(unknown),
                    "跳过 Anthropic 未来未知事件"
                );
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Completion> {
        if !self.completed {
            bail!("Anthropic Messages stream ended before message_stop");
        }
        let stop_reason = self.stop_reason.ok_or_else(|| {
            color_eyre::eyre::eyre!("Anthropic Messages stream is missing stop_reason")
        })?;
        let prompt = self.input_tokens.saturating_add(self.cache_tokens);
        Ok(Completion {
            usage: Some(Usage {
                prompt_tokens: u32_token(Some(prompt)),
                completion_tokens: u32_token(Some(self.output_tokens)),
                total_tokens: u32_token(Some(prompt.saturating_add(self.output_tokens))),
            }),
            stop_reason,
            tool_calls: Vec::new(),
            provider_state: None,
        })
    }
}

async fn generate_title(request: AdapterTitleRequest<'_>) -> Result<String> {
    let transport = transport::HttpTransport::new(
        request.provider,
        request.proxy,
        &request.settings.parameters,
    )?;
    let body = serde_json::json!({
        "model": request.model,
        "system": request.instructions,
        "messages": [{ "role": "user", "content": request.dialog }],
        "max_tokens": 64,
    });
    let http_request = transport
        .client()
        .post(transport.endpoint("messages")?)
        .json(&body)
        .build()?;
    let response = transport.send(http_request, "Anthropic title").await?;
    let payload = transport.json(response, "Anthropic title").await?;
    let content = payload
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| color_eyre::eyre::eyre!("Anthropic title response is missing content[]"))?;
    let output = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    if output.is_empty() {
        bail!("empty Anthropic title response");
    }
    Ok(output)
}

async fn fetch_models(provider: &Provider, proxy: &ProxySettings) -> Result<ModelCatalog> {
    let parameters = ModelParameters::default();
    let transport = transport::HttpTransport::new(provider, proxy, &parameters)?;
    let mut after_id: Option<String> = None;
    let mut catalog = Vec::new();
    for _ in 0..100 {
        let mut endpoint = transport.endpoint("models")?;
        {
            let mut query = endpoint.query_pairs_mut();
            query.append_pair("limit", "1000");
            if let Some(after_id) = &after_id {
                query.append_pair("after_id", after_id);
            }
        }
        let http_request = transport.client().get(endpoint).build()?;
        let response = transport.send(http_request, "Anthropic model list").await?;
        let payload = transport.json(response, "Anthropic model list").await?;
        let data = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| color_eyre::eyre::eyre!("Anthropic model list is missing data[]"))?;
        for model in data {
            let id = model
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| color_eyre::eyre::eyre!("Anthropic model entry is missing id"))?;
            let capabilities_value = model.get("capabilities").unwrap_or(model);
            catalog.push(ModelInfo {
                id: id.trim().to_owned(),
                capabilities: ModelCapabilities {
                    text: CapabilitySupport::Supported,
                    vision: bool_capability(capabilities_value, &["vision", "image_input"]),
                    audio: bool_capability(capabilities_value, &["audio"]),
                    reasoning: bool_capability(capabilities_value, &["reasoning", "thinking"]),
                    structured_output: bool_capability(
                        capabilities_value,
                        &["structured_output", "json_schema"],
                    ),
                    streaming: CapabilitySupport::Supported,
                    context_window: u32_field(
                        model,
                        &["max_input_tokens", "context_window", "contextWindow"],
                    ),
                    max_output_tokens: u32_field(
                        model,
                        &["max_tokens", "max_output_tokens", "maxOutputTokens"],
                    ),
                },
            });
        }
        let has_more = payload
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_more {
            return Ok(catalog);
        }
        after_id = payload
            .get("last_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if after_id.is_none() {
            bail!("Anthropic model list has_more=true but last_id is missing");
        }
    }
    bail!("Anthropic model list exceeded 100 pages")
}

fn u32_field(value: &Value, names: &[&str]) -> Option<u32> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_u64)
            .map(|number| number.min(u32::MAX as u64) as u32)
    })
}

fn bool_capability(value: &Value, names: &[&str]) -> CapabilitySupport {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_bool))
        .map(|supported| {
            if supported {
                CapabilitySupport::Supported
            } else {
                CapabilitySupport::Unsupported
            }
        })
        .unwrap_or(CapabilitySupport::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::model::SseDecoder;

    fn collect_text(output: &mut String, event: ModelStreamEvent) {
        if let ModelStreamEvent::AssistantTextDelta { text } = event {
            output.push_str(&text);
        }
    }

    #[test]
    fn fixture_covers_ping_text_usage_stop_and_future_event() {
        let raw = include_str!("../../../tests/fixtures/anthropic_messages.sse");
        let mut decoder = SseDecoder::default();
        let mut events = decoder.feed(raw.as_bytes()).unwrap();
        events.extend(decoder.finish().unwrap());
        let mut state = AnthropicStreamState::default();
        let mut output = String::new();
        for event in events {
            state
                .handle(event, |event| collect_text(&mut output, event))
                .unwrap();
        }
        let completion = state.finish().unwrap();
        assert_eq!(output, "Hello");
        assert_eq!(completion.stop_reason, StopReason::Stop);
        assert_eq!(completion.usage.unwrap().total_tokens, 12);
    }

    #[test]
    fn known_event_missing_required_field_is_an_error() {
        let event = SseEvent {
            event: Some("message_delta".to_owned()),
            data: r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":1}}"#.to_owned(),
        };
        assert!(
            AnthropicStreamState::default()
                .handle(event, |_| {})
                .is_err()
        );
    }

    #[test]
    fn error_event_and_early_end_are_reported() {
        let event = SseEvent {
            event: Some("error".to_owned()),
            data: r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#
                .to_owned(),
        };
        assert!(
            AnthropicStreamState::default()
                .handle(event, |_| {})
                .is_err()
        );
        assert!(AnthropicStreamState::default().finish().is_err());
    }

    #[test]
    fn thinking_is_emitted_and_non_chat_blocks_are_ignored() {
        let mut state = AnthropicStreamState::default();
        let mut emitted = Vec::new();
        for event in [
            SseEvent {
                event: Some("content_block_start".to_owned()),
                data: r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"search","input":{}}}"#
                    .to_owned(),
            },
            SseEvent {
                event: Some("content_block_delta".to_owned()),
                data: r#"{"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"plan"}}"#
                    .to_owned(),
            },
            SseEvent {
                event: Some("content_block_delta".to_owned()),
                data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":1}"}}"#
                    .to_owned(),
            },
        ] {
            state.handle(event, |event| emitted.push(event)).unwrap();
        }
        assert!(emitted.iter().any(|event| matches!(
            event,
            ModelStreamEvent::ReasoningDelta { text } if text == "plan"
        )));
        assert_eq!(emitted.len(), 1);
    }

    #[test]
    fn messages_encode_only_conversation_text() {
        let user = Message::user("hello".to_owned(), None);
        let messages = anthropic_messages(&[user]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["type"], "text");
        assert_eq!(messages[0]["content"][0]["text"], "hello");
    }
}
