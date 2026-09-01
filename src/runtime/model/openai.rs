//! OpenAI Chat Completions / Responses adapters and strict compatibility layer.

use async_openai::types::responses;
use color_eyre::eyre::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

use super::{
    AdapterStreamRequest, AdapterTitleRequest, BoxFuture, Completion, ModelCatalog, ModelInfo,
    ModelStreamEvent, ProviderAdapter, SseEvent, StopReason, StreamSink, Usage, apply_extra_body,
    diagnostic_label, object_mut, transport,
};
use crate::config::{
    CapabilitySupport, ModelCapabilities, ModelParameters, Provider, ProxySettings,
    ReasoningSettings,
};
use crate::runtime::tool::{ToolCall, ToolDefinition, ToolRound};
use crate::session::{Message, Role};

pub(crate) static CHAT_ADAPTER: OpenAiAdapter = OpenAiAdapter {
    protocol: OpenAiProtocol::ChatCompletions,
};
pub(crate) static RESPONSES_ADAPTER: OpenAiAdapter = OpenAiAdapter {
    protocol: OpenAiProtocol::Responses,
};

#[derive(Clone, Copy)]
enum OpenAiProtocol {
    ChatCompletions,
    Responses,
}

pub(crate) struct OpenAiAdapter {
    protocol: OpenAiProtocol,
}

impl ProviderAdapter for OpenAiAdapter {
    fn stream_reply<'a>(
        &'a self,
        request: AdapterStreamRequest<'a>,
        sink: StreamSink,
    ) -> BoxFuture<'a, Completion> {
        Box::pin(async move {
            match self.protocol {
                OpenAiProtocol::ChatCompletions => stream_chat(request, sink).await,
                OpenAiProtocol::Responses => stream_responses(request, sink).await,
            }
        })
    }

    fn generate_title<'a>(&'a self, request: AdapterTitleRequest<'a>) -> BoxFuture<'a, String> {
        Box::pin(async move {
            match self.protocol {
                OpenAiProtocol::ChatCompletions => title_chat(request).await,
                OpenAiProtocol::Responses => title_responses(request).await,
            }
        })
    }

    fn fetch_models<'a>(
        &'a self,
        provider: &'a Provider,
        proxy: &'a ProxySettings,
    ) -> BoxFuture<'a, ModelCatalog> {
        Box::pin(async move { fetch_models(provider, proxy).await })
    }
}

async fn stream_chat(request: AdapterStreamRequest<'_>, sink: StreamSink) -> Result<Completion> {
    let transport = transport::HttpTransport::with_cancellation(
        request.provider,
        request.proxy,
        &request.settings.parameters,
        request.cancellation.clone(),
    )?;
    let mut messages = openai_chat_messages(request.history);
    if let Some(compacted_context) = request.compacted_context {
        messages.insert(
            0,
            serde_json::json!({ "role": "user", "content": compacted_context }),
        );
    }
    append_chat_tool_rounds(&mut messages, request.tool_rounds)?;
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !request.tools.is_empty() {
        object_mut(&mut body)?.insert(
            "tools".to_owned(),
            Value::Array(openai_chat_tools(request.tools)),
        );
    }
    apply_common_parameters(
        &mut body,
        &request.settings.parameters,
        OpenAiProtocol::ChatCompletions,
    )?;
    apply_extra_body(&mut body, &request.settings.parameters.extra_body);
    let http_request = transport
        .client()
        .post(transport.endpoint("chat/completions")?)
        .json(&body)
        .build()
        .context("failed to build chat/completions request")?;
    let response = transport.send(http_request, "chat/completions").await?;
    sink.record_request_id(transport::response_request_id(response.headers()));
    let mut state = ChatStreamState::default();
    transport::consume_sse(
        response,
        transport.policy.idle_timeout,
        request.cancellation,
        |event| state.handle(event, |event| sink.emit(event)),
    )
    .await?;
    state.finish()
}

async fn stream_responses(
    request: AdapterStreamRequest<'_>,
    sink: StreamSink,
) -> Result<Completion> {
    let transport = transport::HttpTransport::with_cancellation(
        request.provider,
        request.proxy,
        &request.settings.parameters,
        request.cancellation.clone(),
    )?;
    let mut items = openai_response_items(request.history);
    if let Some(compacted_context) = request.compacted_context {
        items.insert(
            0,
            serde_json::json!({
                "type": "message",
                "role": "user",
                "content": compacted_context,
            }),
        );
    }
    append_response_tool_rounds(&mut items, request.tool_rounds)?;
    let mut body = serde_json::json!({
        "model": request.model,
        "input": items,
        "stream": true,
    });
    if !request.tools.is_empty() {
        let object = object_mut(&mut body)?;
        object.insert(
            "tools".to_owned(),
            Value::Array(openai_response_tools(request.tools)),
        );
        object.insert(
            "include".to_owned(),
            serde_json::json!(["reasoning.encrypted_content"]),
        );
    }
    apply_common_parameters(
        &mut body,
        &request.settings.parameters,
        OpenAiProtocol::Responses,
    )?;
    apply_extra_body(&mut body, &request.settings.parameters.extra_body);
    let http_request = transport
        .client()
        .post(transport.endpoint("responses")?)
        .json(&body)
        .build()
        .context("failed to build responses request")?;
    let response = transport.send(http_request, "responses").await?;
    sink.record_request_id(transport::response_request_id(response.headers()));
    let mut state = ResponsesStreamState::default();
    transport::consume_sse(
        response,
        transport.policy.idle_timeout,
        request.cancellation,
        |event| state.handle(event, |event| sink.emit(event)),
    )
    .await?;
    state.finish()
}

fn openai_chat_messages(history: &[Message]) -> Vec<Value> {
    history
        .iter()
        .map(|message| {
            serde_json::json!({
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": message.content,
            })
        })
        .collect()
}

fn openai_response_items(history: &[Message]) -> Vec<Value> {
    history
        .iter()
        .filter(|message| message.role == Role::User || !message.content.is_empty())
        .map(|message| {
            serde_json::json!({
                "type": "message",
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": message.content,
            })
        })
        .collect()
}

fn openai_chat_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut function = serde_json::json!({
                "name": tool.model_name,
                "parameters": tool.input_schema,
            });
            if let Some(description) = &tool.description {
                function["description"] = Value::String(description.clone());
            }
            serde_json::json!({
                "type": "function",
                "function": function,
            })
        })
        .collect()
}

fn openai_response_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut definition = serde_json::json!({
                "type": "function",
                "name": tool.model_name,
                "parameters": tool.input_schema,
            });
            if let Some(description) = &tool.description {
                definition["description"] = Value::String(description.clone());
            }
            definition
        })
        .collect()
}

fn append_chat_tool_rounds(messages: &mut Vec<Value>, rounds: &[ToolRound]) -> Result<()> {
    for round in rounds {
        let mut legacy_function_call = false;
        if let Some(state) = round
            .provider_state
            .as_ref()
            .filter(|state| state.is_object())
        {
            legacy_function_call = state.get("function_call").is_some();
            messages.push(state.clone());
        } else {
            let calls = round
                .calls
                .iter()
                .map(|call| {
                    Ok(serde_json::json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments)?,
                        },
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": calls,
            }));
        }
        for result in &round.results {
            if legacy_function_call {
                messages.push(serde_json::json!({
                    "role": "function",
                    "name": result.name,
                    "content": serde_json::to_string(&result.output)?,
                }));
            } else {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": result.call_id,
                    "content": serde_json::to_string(&result.output)?,
                }));
            }
        }
    }
    Ok(())
}

fn append_response_tool_rounds(items: &mut Vec<Value>, rounds: &[ToolRound]) -> Result<()> {
    for round in rounds {
        if let Some(state) = round.provider_state.as_ref().and_then(Value::as_array) {
            items.extend(state.iter().cloned());
        } else {
            for call in &round.calls {
                items.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": call.provider_id.as_deref().unwrap_or(&call.id),
                    "name": call.name,
                    "arguments": serde_json::to_string(&call.arguments)?,
                }));
            }
        }
        for result in &round.results {
            let provider_call_id = round
                .calls
                .iter()
                .find(|call| call.id == result.call_id)
                .and_then(|call| call.provider_id.as_deref())
                .unwrap_or(&result.call_id);
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": provider_call_id,
                "output": serde_json::to_string(&result.output)?,
            }));
        }
    }
    Ok(())
}

fn apply_common_parameters(
    body: &mut Value,
    parameters: &ModelParameters,
    protocol: OpenAiProtocol,
) -> Result<()> {
    let object = object_mut(body)?;
    if let Some(temperature) = parameters.temperature {
        object.insert("temperature".to_owned(), Value::from(temperature));
    }
    if let Some(top_p) = parameters.top_p {
        object.insert("top_p".to_owned(), Value::from(top_p));
    }
    if let Some(max_output_tokens) = parameters.max_output_tokens {
        let key = match protocol {
            OpenAiProtocol::ChatCompletions => "max_completion_tokens",
            OpenAiProtocol::Responses => "max_output_tokens",
        };
        object.insert(key.to_owned(), Value::from(max_output_tokens));
    }
    if let Some(ReasoningSettings::OpenAi { effort }) = &parameters.reasoning {
        match protocol {
            OpenAiProtocol::ChatCompletions => {
                object.insert("reasoning_effort".to_owned(), Value::from(effort.clone()));
            }
            OpenAiProtocol::Responses => {
                object.insert(
                    "reasoning".to_owned(),
                    serde_json::json!({ "effort": effort }),
                );
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    delta: ChatDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCallDelta>,
    #[serde(default)]
    function_call: Option<ChatFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

#[derive(Default)]
struct ChatStreamState {
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
    saw_done: bool,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    assistant_text: String,
    reasoning_content: String,
    reasoning: String,
    tool_finish: bool,
    legacy_function_call: bool,
}

impl ChatStreamState {
    fn handle<F>(&mut self, event: SseEvent, mut emit: F) -> Result<()>
    where
        F: FnMut(ModelStreamEvent),
    {
        if let Some(name) = event.event.as_deref()
            && name != "message"
        {
            bail!(
                "unknown chat/completions SSE event {:?}",
                diagnostic_label(name)
            );
        }
        if event.data.trim() == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }
        let chunk: ChatChunk =
            serde_json::from_str(&event.data).context("invalid chat/completions stream payload")?;
        if let Some(usage) = chunk.usage {
            self.usage = Some(Usage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            });
        }
        for choice in chunk.choices {
            if let Some(text) = choice.delta.content {
                self.assistant_text.push_str(&text);
                emit(ModelStreamEvent::AssistantTextDelta { text });
            }
            if let Some(text) = choice.delta.reasoning_content {
                self.reasoning_content.push_str(&text);
                emit(ModelStreamEvent::ReasoningDelta { text });
            }
            if let Some(text) = choice.delta.reasoning {
                self.reasoning.push_str(&text);
                emit(ModelStreamEvent::ReasoningDelta { text });
            }
            for (position, delta) in choice.delta.tool_calls.into_iter().enumerate() {
                if delta.id.is_none() && delta.function.is_none() {
                    continue;
                }
                let partial = self
                    .tool_calls
                    .entry(delta.index.unwrap_or(position))
                    .or_default();
                if let Some(id) = delta.id {
                    partial.id.push_str(&id);
                }
                if let Some(function) = delta.function {
                    if let Some(name) = function.name {
                        partial.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments {
                        partial.arguments.push_str(&arguments);
                    }
                }
            }
            if let Some(function) = choice.delta.function_call {
                self.legacy_function_call = true;
                let partial = self.tool_calls.entry(0).or_default();
                if partial.id.is_empty() {
                    partial.id = "legacy_function_call".to_owned();
                }
                if let Some(name) = function.name {
                    partial.name.push_str(&name);
                }
                if let Some(arguments) = function.arguments {
                    partial.arguments.push_str(&arguments);
                }
            }
            if let Some(reason) = choice.finish_reason {
                let reason = match reason.as_str() {
                    "stop" => StopReason::Stop,
                    "length" => StopReason::MaxOutputTokens,
                    "content_filter" => StopReason::ContentFilter,
                    "tool_calls" | "function_call" => {
                        self.tool_finish = true;
                        StopReason::Other(diagnostic_label(&reason))
                    }
                    unknown => bail!(
                        "unknown chat/completions finish_reason {:?}",
                        diagnostic_label(unknown)
                    ),
                };
                self.stop_reason = Some(reason);
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Completion> {
        if !self.saw_done {
            bail!("chat/completions stream ended before [DONE]");
        }
        let stop_reason = self
            .stop_reason
            .ok_or_else(|| color_eyre::eyre::eyre!("chat stream ended before a finish marker"))?;
        let tool_calls = finish_tool_calls(self.tool_calls, "chat")?;
        if self.tool_finish && tool_calls.is_empty() {
            bail!("chat stream finished with tool_calls but returned no tool call");
        }
        if self.legacy_function_call && tool_calls.len() != 1 {
            bail!("legacy chat function_call must contain exactly one call");
        }
        let provider_state = (!tool_calls.is_empty()).then(|| {
            let mut state = serde_json::json!({
                "role": "assistant",
                "content": if self.assistant_text.is_empty() {
                    Value::Null
                } else {
                    Value::String(self.assistant_text)
                },
            });
            if self.legacy_function_call {
                let call = &tool_calls[0];
                state["function_call"] = serde_json::json!({
                    "name": call.name,
                    "arguments": call.arguments.to_string(),
                });
            } else {
                state["tool_calls"] = Value::Array(
                    tool_calls
                        .iter()
                        .map(|call| {
                            serde_json::json!({
                                "id": call.provider_id.as_deref().unwrap_or(&call.id),
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments.to_string(),
                                },
                            })
                        })
                        .collect(),
                );
            }
            if !self.reasoning_content.is_empty() {
                state["reasoning_content"] = Value::String(self.reasoning_content);
            }
            if !self.reasoning.is_empty() {
                state["reasoning"] = Value::String(self.reasoning);
            }
            state
        });
        Ok(Completion {
            usage: self.usage,
            stop_reason,
            tool_calls,
            provider_state,
        })
    }
}

fn finish_tool_calls(
    partials: BTreeMap<usize, PartialToolCall>,
    protocol: &str,
) -> Result<Vec<ToolCall>> {
    partials
        .into_iter()
        .map(|(index, partial)| {
            if partial.name.trim().is_empty() {
                bail!("{protocol} tool call {index} is missing a function name");
            }
            let arguments = if partial.arguments.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&partial.arguments).with_context(|| {
                    format!("{protocol} tool call {index} has invalid JSON arguments")
                })?
            };
            let id = if partial.id.is_empty() {
                format!("{protocol}_call_{index}")
            } else {
                partial.id
            };
            Ok(ToolCall {
                provider_id: Some(id.clone()),
                id,
                name: partial.name,
                arguments,
            })
        })
        .collect()
}

#[derive(Default)]
struct ResponsesStreamState {
    usage: Option<Usage>,
    completed: bool,
    refusal: bool,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    provider_state: Option<Value>,
}

impl ResponsesStreamState {
    fn handle<F>(&mut self, event: SseEvent, mut emit: F) -> Result<()>
    where
        F: FnMut(ModelStreamEvent),
    {
        if event.data.trim() == "[DONE]" {
            return Ok(());
        }
        let payload: Value =
            serde_json::from_str(&event.data).context("invalid Responses API stream JSON")?;
        let payload_type = payload.get("type").and_then(Value::as_str);
        if payload_type.is_some_and(|event_type| event_type.chars().count() > 128) {
            bail!("Responses API event type exceeds diagnostic limit");
        }
        if let Some(event_name) = event.event.as_deref()
            && event_name != "message"
            && Some(event_name) != payload_type
        {
            bail!(
                "Responses SSE event name {event_name:?} does not match payload type {payload_type:?}"
            );
        }
        emit_responses_semantic_delta(&payload, &mut emit)?;
        let Some(event) = deserialize_responses_stream_payload(payload)
            .context("invalid Responses API stream event")?
        else {
            return Ok(());
        };
        match event {
            responses::ResponseStreamEvent::ResponseOutputTextDelta(delta) => {
                emit(ModelStreamEvent::AssistantTextDelta { text: delta.delta })
            }
            responses::ResponseStreamEvent::ResponseRefusalDelta(delta) => {
                self.refusal = true;
                emit(ModelStreamEvent::AssistantTextDelta { text: delta.delta });
            }
            responses::ResponseStreamEvent::ResponseOutputItemAdded(event) => {
                if let responses::OutputItem::FunctionCall(call) = event.item {
                    self.record_function_call(
                        event.output_index as usize,
                        call.call_id,
                        call.name,
                        call.arguments,
                    );
                }
            }
            responses::ResponseStreamEvent::ResponseOutputItemDone(event) => {
                if let responses::OutputItem::FunctionCall(call) = event.item {
                    self.record_function_call(
                        event.output_index as usize,
                        call.call_id,
                        call.name,
                        call.arguments,
                    );
                }
            }
            responses::ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(event) => {
                self.tool_calls
                    .entry(event.output_index as usize)
                    .or_default()
                    .arguments
                    .push_str(&event.delta);
            }
            responses::ResponseStreamEvent::ResponseFunctionCallArgumentsDone(event) => {
                let partial = self
                    .tool_calls
                    .entry(event.output_index as usize)
                    .or_default();
                partial.arguments = event.arguments;
                if let Some(name) = event.name {
                    partial.name = name;
                }
            }
            responses::ResponseStreamEvent::ResponseCompleted(event) => {
                self.provider_state = Some(
                    serde_json::to_value(&event.response.output)
                        .context("failed to preserve Responses output items")?,
                );
                for (index, item) in event.response.output.iter().enumerate() {
                    if let responses::OutputItem::FunctionCall(call) = item {
                        self.record_function_call(
                            index,
                            call.call_id.clone(),
                            call.name.clone(),
                            call.arguments.clone(),
                        );
                    }
                }
                self.usage = event.response.usage.map(|usage| Usage {
                    prompt_tokens: usage.input_tokens,
                    completion_tokens: usage.output_tokens,
                    total_tokens: usage.total_tokens,
                });
                self.completed = true;
            }
            responses::ResponseStreamEvent::ResponseFailed(_) => {
                bail!("Responses API reported response.failed")
            }
            responses::ResponseStreamEvent::ResponseIncomplete(event) => {
                let detail = event
                    .response
                    .incomplete_details
                    .map(|details| details.reason)
                    .unwrap_or_else(|| "Responses API reported response.incomplete".to_owned());
                bail!(detail);
            }
            responses::ResponseStreamEvent::ResponseError(_) => {
                bail!("Responses API reported an error event")
            }
            _ => {}
        }
        Ok(())
    }

    fn record_function_call(
        &mut self,
        index: usize,
        call_id: String,
        name: String,
        arguments: String,
    ) {
        let partial = self.tool_calls.entry(index).or_default();
        partial.id = call_id;
        partial.name = name;
        partial.arguments = arguments;
    }

    fn finish(self) -> Result<Completion> {
        if !self.completed {
            bail!("Responses stream ended before response.completed");
        }
        let tool_calls = finish_tool_calls(self.tool_calls, "responses")?;
        Ok(Completion {
            usage: self.usage,
            stop_reason: if self.refusal {
                StopReason::Refusal
            } else {
                StopReason::Stop
            },
            tool_calls,
            provider_state: self.provider_state,
        })
    }
}

fn emit_responses_semantic_delta<F>(payload: &Value, emit: &mut F) -> Result<()>
where
    F: FnMut(ModelStreamEvent),
{
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            let text = payload
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    color_eyre::eyre::eyre!("Responses reasoning delta is missing delta")
                })?;
            emit(ModelStreamEvent::ReasoningDelta {
                text: text.to_owned(),
            });
        }
        "response.queued" => {
            emit(ModelStreamEvent::System {
                message: event_type.to_owned(),
            });
        }
        _ => {}
    }
    Ok(())
}

/// 只修正两个登记过的兼容偏差：空 truncation 和 OpenCode `ping`。
fn deserialize_responses_payload<T: DeserializeOwned>(mut payload: Value) -> serde_json::Result<T> {
    let payload_type = payload
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| payload.get("object").and_then(Value::as_str))
        .map(str::to_owned);
    if normalize_empty_truncation(&mut payload) {
        tracing::debug!(
            payload_type = ?payload_type,
            compatibility_rule = "responses.empty_truncation",
            "将 Responses API 的空 truncation 归一化为 disabled"
        );
    }
    serde_json::from_value(payload)
}

fn deserialize_responses_stream_payload(
    payload: Value,
) -> serde_json::Result<Option<responses::ResponseStreamEvent>> {
    if payload.get("type").and_then(Value::as_str) == Some("ping") {
        tracing::trace!(
            compatibility_rule = "opencode.responses.ping",
            "忽略 Responses API 流心跳"
        );
        return Ok(None);
    }
    deserialize_responses_payload(payload).map(Some)
}

fn normalize_empty_truncation(payload: &mut Value) -> bool {
    if let Some(response) = payload.get_mut("response") {
        return normalize_response_truncation(response);
    }
    normalize_response_truncation(payload)
}

fn normalize_response_truncation(response: &mut Value) -> bool {
    let Some(truncation) = response.get_mut("truncation") else {
        return false;
    };
    if truncation.as_str() != Some("") {
        return false;
    }
    *truncation = Value::String("disabled".to_owned());
    true
}

async fn title_chat(request: AdapterTitleRequest<'_>) -> Result<String> {
    let transport = transport::HttpTransport::new(
        request.provider,
        request.proxy,
        &request.settings.parameters,
    )?;
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": [
            { "role": "system", "content": request.instructions },
            { "role": "user", "content": request.dialog }
        ],
        "max_completion_tokens": 64,
    });
    if let Some(temperature) = request.settings.parameters.temperature {
        object_mut(&mut body)?.insert("temperature".to_owned(), Value::from(temperature));
    }
    let http_request = transport
        .client()
        .post(transport.endpoint("chat/completions")?)
        .json(&body)
        .build()?;
    let response = transport
        .send(http_request, "title chat/completions")
        .await?;
    let payload = transport.json(response, "title chat/completions").await?;
    payload
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| color_eyre::eyre::eyre!("empty title response"))
}

async fn title_responses(request: AdapterTitleRequest<'_>) -> Result<String> {
    let transport = transport::HttpTransport::new(
        request.provider,
        request.proxy,
        &request.settings.parameters,
    )?;
    let body = serde_json::json!({
        "model": request.model,
        "instructions": request.instructions,
        "input": request.dialog,
        "max_output_tokens": 64,
    });
    let http_request = transport
        .client()
        .post(transport.endpoint("responses")?)
        .json(&body)
        .build()?;
    let response = transport.send(http_request, "title responses").await?;
    let payload = transport.json(response, "title responses").await?;
    let response: responses::Response =
        deserialize_responses_payload(payload).context("invalid title Responses API payload")?;
    let mut output = String::new();
    for item in response.output {
        if let responses::OutputItem::Message(message) = item {
            for content in message.content {
                if let responses::OutputMessageContent::OutputText(text) = content {
                    output.push_str(&text.text);
                }
            }
        }
    }
    if output.is_empty() {
        bail!("empty title response");
    }
    Ok(output)
}

async fn fetch_models(provider: &Provider, proxy: &ProxySettings) -> Result<ModelCatalog> {
    let parameters = ModelParameters::default();
    let transport = transport::HttpTransport::new(provider, proxy, &parameters)?;
    let request = transport
        .client()
        .get(transport.endpoint("models")?)
        .build()?;
    let response = transport.send(request, "OpenAI model list").await?;
    let payload = transport.json(response, "OpenAI model list").await?;
    let data = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| color_eyre::eyre::eyre!("OpenAI model list is missing data[]"))?;
    let mut catalog = Vec::with_capacity(data.len());
    for model in data {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            bail!("OpenAI model entry is missing id");
        };
        let mut capabilities = ModelCapabilities {
            text: CapabilitySupport::Unknown,
            streaming: CapabilitySupport::Unknown,
            context_window: u32_field(model, &["context_window", "contextWindow"]),
            max_output_tokens: u32_field(model, &["max_output_tokens", "maxOutputTokens"]),
            ..ModelCapabilities::default()
        };
        if let Some(value) = model.get("capabilities") {
            capabilities.vision = bool_capability(value, &["vision", "image"]);
            capabilities.audio = bool_capability(value, &["audio"]);
            capabilities.reasoning = bool_capability(value, &["reasoning"]);
            capabilities.structured_output =
                bool_capability(value, &["structured_output", "json_schema"]);
        }
        catalog.push(ModelInfo {
            id: id.trim().to_owned(),
            capabilities,
        });
    }
    Ok(catalog)
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

    fn decode(raw: &str) -> Vec<SseEvent> {
        let mut decoder = SseDecoder::default();
        let mut events = decoder.feed(raw.as_bytes()).unwrap();
        events.extend(decoder.finish().unwrap());
        events
    }

    fn collect_text(output: &mut String, event: ModelStreamEvent) {
        if let ModelStreamEvent::AssistantTextDelta { text } = event {
            output.push_str(&text);
        }
    }

    #[test]
    fn chat_fixture_covers_delta_usage_finish_and_done() {
        let mut state = ChatStreamState::default();
        let mut output = String::new();
        for event in decode(include_str!("../../../tests/fixtures/openai_chat.sse")) {
            state
                .handle(event, |event| collect_text(&mut output, event))
                .unwrap();
        }
        let completion = state.finish().unwrap();
        assert_eq!(output, "你好");
        assert_eq!(completion.stop_reason, StopReason::Stop);
        assert_eq!(completion.usage.unwrap().total_tokens, 5);
    }

    #[test]
    fn chat_stream_ending_without_done_is_rejected() {
        let mut state = ChatStreamState::default();
        state
            .handle(
                SseEvent {
                    event: None,
                    data: r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":"stop"}]}"#
                        .to_owned(),
                },
                |_| {},
            )
            .unwrap();
        assert!(state.finish().is_err());
    }

    #[test]
    fn responses_fixture_ignores_registered_ping_and_is_otherwise_strict() {
        let mut state = ResponsesStreamState::default();
        let mut output = String::new();
        for event in decode(include_str!("../../../tests/fixtures/openai_responses.sse")) {
            state
                .handle(event, |event| collect_text(&mut output, event))
                .unwrap();
        }
        assert_eq!(output, "hello");
        assert_eq!(state.finish().unwrap().usage.unwrap().total_tokens, 3);

        let unknown = SseEvent {
            event: None,
            data: r#"{"type":"provider.custom_event"}"#.to_owned(),
        };
        assert!(
            ResponsesStreamState::default()
                .handle(unknown, |_| {})
                .is_err()
        );
        assert!(ResponsesStreamState::default().finish().is_err());
    }

    #[test]
    fn empty_truncation_is_the_only_field_value_normalization() {
        let payload = serde_json::json!({
            "created_at": 1,
            "id": "resp_test",
            "model": "test",
            "object": "response",
            "output": [],
            "status": "completed",
            "truncation": ""
        });
        let response: responses::Response = deserialize_responses_payload(payload).unwrap();
        assert_eq!(response.truncation, Some(responses::Truncation::Disabled));
    }

    #[test]
    fn chat_reasoning_is_structured_and_extra_delta_fields_are_ignored() {
        let event = SseEvent {
            event: None,
            data: serde_json::json!({
                "choices": [{
                    "delta": {
                        "reasoning_content": "plan",
                        "tool_calls": [{
                            "id": "call-1",
                            "function": {
                                "name": "search",
                                "arguments": "{\"q\":"
                            }
                        }]
                    }
                }]
            })
            .to_string(),
        };
        let mut emitted = Vec::new();
        ChatStreamState::default()
            .handle(event, |event| emitted.push(event))
            .unwrap();
        assert!(matches!(
            &emitted[0],
            ModelStreamEvent::ReasoningDelta { text } if text == "plan"
        ));
        assert_eq!(emitted.len(), 1);
    }

    #[test]
    fn responses_semantic_mapper_separates_reasoning_and_system_events() {
        let mut emitted = Vec::new();
        emit_responses_semantic_delta(
            &serde_json::json!({
                "type": "response.reasoning_text.delta",
                "delta": "step"
            }),
            &mut |event| emitted.push(event),
        )
        .unwrap();
        emit_responses_semantic_delta(
            &serde_json::json!({ "type": "response.queued" }),
            &mut |event| emitted.push(event),
        )
        .unwrap();
        assert!(matches!(
            &emitted[0],
            ModelStreamEvent::ReasoningDelta { text } if text == "step"
        ));
        assert!(matches!(
            &emitted[1],
            ModelStreamEvent::System { message } if message == "response.queued"
        ));
    }

    #[test]
    fn chat_and_responses_encode_only_conversation_messages() {
        let assistant = Message::user("hello".to_owned(), None);
        let chat = openai_chat_messages(std::slice::from_ref(&assistant));
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0]["content"], "hello");
        assert_eq!(chat[0]["role"], "user");
        let responses = openai_response_items(&[assistant]);
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["type"], "message");
        assert_eq!(responses[0]["content"], "hello");
    }

    #[test]
    fn chat_reassembles_streamed_tool_calls_and_parses_arguments() {
        let mut state = ChatStreamState::default();
        for data in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"mcp_docs__search","arguments":"{\"q\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"rust\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ] {
            state
                .handle(
                    SseEvent {
                        event: None,
                        data: data.to_owned(),
                    },
                    |_| {},
                )
                .unwrap();
        }
        let completion = state.finish().unwrap();
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].id, "call-1");
        assert_eq!(
            completion.tool_calls[0].provider_id.as_deref(),
            Some("call-1")
        );
        assert_eq!(completion.tool_calls[0].name, "mcp_docs__search");
        assert_eq!(
            completion.tool_calls[0].arguments,
            serde_json::json!({"q":"rust"})
        );
    }

    #[test]
    fn responses_parses_function_call_output_items() {
        let mut state = ResponsesStreamState::default();
        state
            .handle(
                SseEvent {
                    event: None,
                    data: serde_json::json!({
                        "type":"response.output_item.done",
                        "sequence_number":1,
                        "output_index":0,
                        "item":{
                            "type":"function_call",
                            "arguments":"{\"city\":\"LA\"}",
                            "call_id":"call-weather",
                            "name":"mcp_exa__weather"
                        }
                    })
                    .to_string(),
                },
                |_| {},
            )
            .unwrap();
        let calls = finish_tool_calls(state.tool_calls, "responses").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-weather");
        assert_eq!(calls[0].provider_id.as_deref(), Some("call-weather"));
        assert_eq!(calls[0].arguments["city"], "LA");
    }

    #[test]
    fn both_openai_protocols_encode_tool_catalog_and_rounds() {
        let tools = vec![ToolDefinition {
            model_name: "mcp_docs__search".to_owned(),
            server_name: "docs".to_owned(),
            remote_name: "search".to_owned(),
            description: Some("Search docs".to_owned()),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        assert_eq!(openai_chat_tools(&tools)[0]["type"], "function");
        assert_eq!(openai_response_tools(&tools)[0]["name"], "mcp_docs__search");

        let round = ToolRound {
            calls: vec![ToolCall {
                id: "call-1".to_owned(),
                provider_id: Some("call-1".to_owned()),
                name: "mcp_docs__search".to_owned(),
                arguments: serde_json::json!({"q":"rust"}),
            }],
            results: vec![crate::runtime::tool::ToolResult {
                call_id: "call-1".to_owned(),
                name: "mcp_docs__search".to_owned(),
                output: serde_json::json!({"answer":"found"}),
                is_error: false,
            }],
            provider_state: None,
        };
        let mut chat = Vec::new();
        append_chat_tool_rounds(&mut chat, std::slice::from_ref(&round)).unwrap();
        assert_eq!(chat[0]["tool_calls"][0]["id"], "call-1");
        assert_eq!(chat[1]["role"], "tool");
        let mut responses = Vec::new();
        append_response_tool_rounds(&mut responses, &[round]).unwrap();
        assert_eq!(responses[0]["type"], "function_call");
        assert_eq!(responses[1]["type"], "function_call_output");
    }
}
