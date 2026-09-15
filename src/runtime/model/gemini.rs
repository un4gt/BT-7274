//! Gemini GenerateContent 原生 adapter。

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
    runtime::tool::{ToolCall, ToolDefinition, ToolRound},
    session::{Message, Role},
};

pub(crate) static GEMINI_ADAPTER: GeminiAdapter = GeminiAdapter;

pub(crate) struct GeminiAdapter;

impl ProviderAdapter for GeminiAdapter {
    fn stream_reply<'a>(
        &'a self,
        request: AdapterStreamRequest<'a>,
        sink: StreamSink,
    ) -> BoxFuture<'a, Completion> {
        Box::pin(async move { stream_generate_content(request, sink).await })
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

async fn stream_generate_content(
    request: AdapterStreamRequest<'_>,
    sink: StreamSink,
) -> Result<Completion> {
    let transport = transport::HttpTransport::with_cancellation(
        request.provider,
        request.proxy,
        &request.settings.parameters,
        request.cancellation.clone(),
    )?;
    let mut contents = gemini_contents(request.history);
    if let Some(compacted_context) = request.compacted_context {
        contents.insert(
            0,
            serde_json::json!({
                "role": "user",
                "parts": [{ "text": compacted_context }],
            }),
        );
    }
    append_gemini_tool_rounds(&mut contents, request.tool_rounds);
    let mut body = serde_json::json!({ "contents": contents });
    if !request.tools.is_empty() {
        object_mut(&mut body)?.insert(
            "tools".to_owned(),
            serde_json::json!([{
                "functionDeclarations": gemini_tools(request.tools),
            }]),
        );
    }
    apply_parameters(&mut body, &request.settings.parameters)?;
    apply_extra_body(&mut body, &request.settings.parameters.extra_body);
    let mut endpoint = model_endpoint(&transport, request.model, "streamGenerateContent")?;
    endpoint.query_pairs_mut().append_pair("alt", "sse");
    let http_request = transport
        .client()
        .post(endpoint)
        .json(&body)
        .build()
        .context("failed to build Gemini streamGenerateContent request")?;
    let response = transport
        .send(http_request, "Gemini streamGenerateContent")
        .await?;
    sink.record_request_id(transport::response_request_id(response.headers()));
    let mut state = GeminiStreamState::default();
    transport::consume_sse(
        response,
        transport.policy.idle_timeout,
        request.cancellation,
        |event| state.handle(event, |event| sink.emit(event)),
    )
    .await?;
    state.finish()
}

fn gemini_contents(history: &[Message]) -> Vec<Value> {
    history
        .iter()
        .filter(|message| message.role == Role::User || !message.content().is_empty())
        .map(|message| {
            serde_json::json!({
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "model",
                },
                "parts": [{"text": message.content()}],
            })
        })
        .collect()
}

fn gemini_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut declaration = serde_json::json!({
                "name": tool.model_name,
                "parameters": tool.input_schema,
            });
            if let Some(description) = &tool.description {
                declaration["description"] = Value::String(description.clone());
            }
            declaration
        })
        .collect()
}

fn append_gemini_tool_rounds(contents: &mut Vec<Value>, rounds: &[ToolRound]) {
    for round in rounds {
        if let Some(state) = round
            .provider_state
            .as_ref()
            .filter(|state| state.is_object())
        {
            contents.push(state.clone());
        } else {
            let calls = round
                .calls
                .iter()
                .map(|call| {
                    let mut function_call = serde_json::json!({
                        "name": call.name,
                        "args": call.arguments,
                    });
                    if let Some(provider_id) = &call.provider_id {
                        function_call["id"] = Value::String(provider_id.clone());
                    }
                    serde_json::json!({ "functionCall": function_call })
                })
                .collect::<Vec<_>>();
            contents.push(serde_json::json!({ "role": "model", "parts": calls }));
        }

        let results = round
            .results
            .iter()
            .map(|result| {
                let mut function_response = serde_json::json!({
                    "name": result.name,
                    "response": {
                        "output": result.output,
                        "isError": result.is_error,
                    },
                });
                if let Some(provider_id) = round
                    .calls
                    .iter()
                    .find(|call| call.id == result.call_id)
                    .and_then(|call| call.provider_id.as_ref())
                {
                    function_response["id"] = Value::String(provider_id.clone());
                }
                serde_json::json!({ "functionResponse": function_response })
            })
            .collect::<Vec<_>>();
        contents.push(serde_json::json!({ "role": "user", "parts": results }));
    }
}

fn apply_parameters(body: &mut Value, parameters: &ModelParameters) -> Result<()> {
    let mut generation = serde_json::Map::new();
    if let Some(temperature) = parameters.temperature {
        generation.insert("temperature".to_owned(), Value::from(temperature));
    }
    if let Some(top_p) = parameters.top_p {
        generation.insert("topP".to_owned(), Value::from(top_p));
    }
    if let Some(max_output_tokens) = parameters.max_output_tokens {
        generation.insert("maxOutputTokens".to_owned(), Value::from(max_output_tokens));
    }
    if let Some(ReasoningSettings::Gemini { thinking_budget }) = &parameters.reasoning {
        generation.insert(
            "thinkingConfig".to_owned(),
            serde_json::json!({ "thinkingBudget": thinking_budget }),
        );
    }
    if !generation.is_empty() {
        object_mut(body)?.insert("generationConfig".to_owned(), Value::Object(generation));
    }
    Ok(())
}

fn model_endpoint(
    transport: &transport::HttpTransport,
    model: &str,
    method: &str,
) -> Result<url::Url> {
    let model = model.trim().strip_prefix("models/").unwrap_or(model.trim());
    if model.is_empty()
        || model
            .chars()
            .any(|character| matches!(character, '?' | '#' | '\\'))
    {
        bail!("invalid Gemini model name");
    }
    transport.endpoint(&format!("models/{model}:{method}"))
}

#[derive(Default)]
struct GeminiStreamState {
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
    tool_calls: Vec<ToolCall>,
    model_parts: Vec<Value>,
}

impl GeminiStreamState {
    fn handle<F>(&mut self, event: SseEvent, mut emit: F) -> Result<()>
    where
        F: FnMut(ModelStreamEvent),
    {
        if let Some(name) = event.event.as_deref()
            && name != "message"
        {
            bail!("unknown Gemini SSE event {:?}", diagnostic_label(name));
        }
        if event.data.trim() == "[DONE]" {
            return Ok(());
        }
        let payload: Value = serde_json::from_str(&event.data)
            .context("invalid Gemini streamGenerateContent JSON")?;
        if payload.get("error").is_some() {
            let status = payload
                .pointer("/error/status")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            bail!("Gemini stream error status={status}");
        }
        if let Some(usage) = payload.get("usageMetadata") {
            let prompt = usage.get("promptTokenCount").and_then(Value::as_u64);
            let completion = usage.get("candidatesTokenCount").and_then(Value::as_u64);
            let total = usage.get("totalTokenCount").and_then(Value::as_u64);
            self.usage = Some(Usage {
                prompt_tokens: u32_token(prompt),
                completion_tokens: u32_token(completion),
                total_tokens: u32_token(total.or_else(|| {
                    Some(
                        prompt
                            .unwrap_or_default()
                            .saturating_add(completion.unwrap_or_default()),
                    )
                })),
            });
        }
        let candidates = payload
            .get("candidates")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for candidate in candidates {
            if let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
            {
                for part in parts {
                    if let Some(replay_part) = gemini_replay_part(part) {
                        self.model_parts.push(replay_part);
                    }
                    if part.get("thought").and_then(Value::as_bool) == Some(true) {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            emit(ModelStreamEvent::ReasoningDelta {
                                text: text.to_owned(),
                            });
                        }
                        continue;
                    }
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        emit(ModelStreamEvent::AssistantTextDelta {
                            text: text.to_owned(),
                        });
                    }
                    if let Some(function_call) = part.get("functionCall") {
                        let name = function_call
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                color_eyre::eyre::eyre!(
                                    "Gemini functionCall is missing a function name"
                                )
                            })?;
                        let arguments = function_call
                            .get("args")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        if !arguments.is_object() {
                            bail!("Gemini functionCall args must be a JSON object");
                        }
                        let provider_id = function_call
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned);
                        let id = provider_id
                            .clone()
                            .unwrap_or_else(|| format!("gemini_call_{}", self.tool_calls.len()));
                        let index = self.tool_calls.len();
                        emit(ModelStreamEvent::ToolCallDelta {
                            round: 0,
                            index,
                            call_id: id.clone(),
                            name: name.to_owned(),
                            arguments: arguments.to_string(),
                            replace: true,
                        });
                        self.tool_calls.push(ToolCall {
                            index,
                            id,
                            provider_id,
                            name: name.to_owned(),
                            arguments,
                        });
                    }
                }
            }
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                self.stop_reason = Some(gemini_stop_reason(reason));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Completion> {
        let stop_reason = self.stop_reason.ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "Gemini stream ended before a candidate finishReason was received"
            )
        })?;
        let provider_state = (!self.tool_calls.is_empty()).then(|| {
            serde_json::json!({
                "role": "model",
                "parts": self.model_parts,
            })
        });
        Ok(Completion {
            usage: self.usage,
            stop_reason,
            tool_calls: self.tool_calls,
            provider_state,
        })
    }
}

fn gemini_replay_part(part: &Value) -> Option<Value> {
    let mut part = part.as_object()?.clone();
    if gemini_part_has_data(&part) {
        return Some(Value::Object(part));
    }

    let has_thought_metadata = part.get("thought").is_some_and(Value::is_boolean)
        || part.get("thoughtSignature").is_some_and(Value::is_string);
    if !has_thought_metadata {
        return None;
    }

    // Some compatible gateways omit the empty text carried by signature-only stream parts.
    part.insert("text".to_owned(), Value::String(String::new()));
    Some(Value::Object(part))
}

fn gemini_part_has_data(part: &serde_json::Map<String, Value>) -> bool {
    const DATA_FIELDS: [&str; 7] = [
        "text",
        "inlineData",
        "functionCall",
        "functionResponse",
        "fileData",
        "executableCode",
        "codeExecutionResult",
    ];
    DATA_FIELDS
        .iter()
        .any(|field| part.get(*field).is_some_and(|value| !value.is_null()))
}

fn gemini_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::MaxOutputTokens,
        "SAFETY"
        | "RECITATION"
        | "LANGUAGE"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_owned()),
    }
}

async fn generate_title(request: AdapterTitleRequest<'_>) -> Result<String> {
    let transport = transport::HttpTransport::new(
        request.provider,
        request.proxy,
        &request.settings.parameters,
    )?;
    let body = serde_json::json!({
        "systemInstruction": { "parts": [{ "text": request.instructions }] },
        "contents": [{ "role": "user", "parts": [{ "text": request.dialog }] }],
        "generationConfig": { "maxOutputTokens": 64 },
    });
    let http_request = transport
        .client()
        .post(model_endpoint(
            &transport,
            request.model,
            "generateContent",
        )?)
        .json(&body)
        .build()?;
    let response = transport.send(http_request, "Gemini title").await?;
    let payload = transport.json(response, "Gemini title").await?;
    let candidates = payload
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| color_eyre::eyre::eyre!("Gemini title response is missing candidates[]"))?;
    let output = candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
        })
        .flatten()
        .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    if output.is_empty() {
        bail!("empty Gemini title response");
    }
    Ok(output)
}

async fn fetch_models(provider: &Provider, proxy: &ProxySettings) -> Result<ModelCatalog> {
    let parameters = ModelParameters::default();
    let transport = transport::HttpTransport::new(provider, proxy, &parameters)?;
    let mut page_token: Option<String> = None;
    let mut catalog = Vec::new();
    for _ in 0..100 {
        let mut endpoint = transport.endpoint("models")?;
        {
            let mut query = endpoint.query_pairs_mut();
            query.append_pair("pageSize", "1000");
            if let Some(page_token) = &page_token {
                query.append_pair("pageToken", page_token);
            }
        }
        let http_request = transport.client().get(endpoint).build()?;
        let response = transport.send(http_request, "Gemini model list").await?;
        let payload = transport.json(response, "Gemini model list").await?;
        let models = payload
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| color_eyre::eyre::eyre!("Gemini model list is missing models[]"))?;
        for model in models {
            let name = model
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| color_eyre::eyre::eyre!("Gemini model entry is missing name"))?;
            let methods = model
                .get("supportedGenerationMethods")
                .and_then(Value::as_array);
            let generate_support = methods.map_or(CapabilitySupport::Unknown, |methods| {
                if methods.iter().any(|method| {
                    matches!(
                        method.as_str(),
                        Some("generateContent") | Some("streamGenerateContent")
                    )
                }) {
                    CapabilitySupport::Supported
                } else {
                    CapabilitySupport::Unsupported
                }
            });
            let thinking = match model.get("thinking") {
                Some(Value::Bool(true)) | Some(Value::Object(_)) => CapabilitySupport::Supported,
                Some(Value::Bool(false)) => CapabilitySupport::Unsupported,
                _ => CapabilitySupport::Unknown,
            };
            catalog.push(ModelInfo {
                id: name
                    .strip_prefix("models/")
                    .unwrap_or(name)
                    .trim()
                    .to_owned(),
                capabilities: ModelCapabilities {
                    text: generate_support,
                    vision: CapabilitySupport::Unknown,
                    audio: CapabilitySupport::Unknown,
                    reasoning: thinking,
                    structured_output: CapabilitySupport::Unknown,
                    streaming: generate_support,
                    context_window: u32_field(model, "inputTokenLimit"),
                    max_output_tokens: u32_field(model, "outputTokenLimit"),
                },
            });
        }
        page_token = payload
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        if page_token.is_none() {
            return Ok(catalog);
        }
    }
    bail!("Gemini model list exceeded 100 pages")
}

fn u32_field(value: &Value, name: &str) -> Option<u32> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .map(|number| number.min(u32::MAX as u64) as u32)
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
    fn latest_stream_fixture_covers_incremental_text_usage_and_finish_reason() {
        let raw = include_str!("../../../tests/fixtures/gemini_generate_content.sse");
        let mut decoder = SseDecoder::default();
        let mut events = decoder.feed(raw.as_bytes()).unwrap();
        events.extend(decoder.finish().unwrap());
        let mut state = GeminiStreamState::default();
        let mut output = String::new();
        for event in events {
            state
                .handle(event, |event| collect_text(&mut output, event))
                .unwrap();
        }
        let completion = state.finish().unwrap();
        assert_eq!(output, "Gemini");
        assert_eq!(completion.stop_reason, StopReason::Stop);
        assert_eq!(completion.usage.unwrap().total_tokens, 11);
    }

    #[test]
    fn current_finish_reasons_are_mapped_without_failure() {
        let cases = [
            (
                "FINISH_REASON_UNSPECIFIED",
                StopReason::Other("FINISH_REASON_UNSPECIFIED".to_owned()),
            ),
            ("STOP", StopReason::Stop),
            ("MAX_TOKENS", StopReason::MaxOutputTokens),
            ("SAFETY", StopReason::ContentFilter),
            ("RECITATION", StopReason::ContentFilter),
            ("LANGUAGE", StopReason::ContentFilter),
            ("OTHER", StopReason::Other("OTHER".to_owned())),
            ("BLOCKLIST", StopReason::ContentFilter),
            ("PROHIBITED_CONTENT", StopReason::ContentFilter),
            ("SPII", StopReason::ContentFilter),
            (
                "MALFORMED_FUNCTION_CALL",
                StopReason::Other("MALFORMED_FUNCTION_CALL".to_owned()),
            ),
            ("IMAGE_SAFETY", StopReason::ContentFilter),
            (
                "UNEXPECTED_TOOL_CALL",
                StopReason::Other("UNEXPECTED_TOOL_CALL".to_owned()),
            ),
            (
                "TOO_MANY_TOOL_CALLS",
                StopReason::Other("TOO_MANY_TOOL_CALLS".to_owned()),
            ),
            ("IMAGE_PROHIBITED_CONTENT", StopReason::ContentFilter),
            ("NO_IMAGE", StopReason::Other("NO_IMAGE".to_owned())),
            ("IMAGE_RECITATION", StopReason::ContentFilter),
            ("IMAGE_OTHER", StopReason::Other("IMAGE_OTHER".to_owned())),
        ];

        for (reason, expected) in cases {
            let event = SseEvent {
                event: None,
                data: serde_json::json!({
                    "candidates": [{ "finishReason": reason }],
                })
                .to_string(),
            };
            let mut state = GeminiStreamState::default();
            state.handle(event, |_| {}).unwrap();
            assert_eq!(state.finish().unwrap().stop_reason, expected, "{reason}");
        }
    }

    #[test]
    fn unknown_finish_reason_preserves_partial_reply_and_degrades_to_other() {
        let event = SseEvent {
            event: None,
            data: r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"partial"}]},"finishReason":"NEW_REASON"}]}"#
                .to_owned(),
        };
        let mut state = GeminiStreamState::default();
        let mut output = String::new();

        state
            .handle(event, |event| collect_text(&mut output, event))
            .unwrap();
        let completion = state.finish().unwrap();

        assert_eq!(output, "partial");
        assert_eq!(
            completion.stop_reason,
            StopReason::Other("NEW_REASON".to_owned())
        );
    }

    #[test]
    fn streamed_error_is_reported_without_response_body() {
        let event = SseEvent {
            event: None,
            data: r#"{"error":{"status":"RESOURCE_EXHAUSTED","message":"private"}}"#.to_owned(),
        };
        let error = GeminiStreamState::default()
            .handle(event, |_| {})
            .unwrap_err();
        let detail = error.to_string();
        assert!(detail.contains("RESOURCE_EXHAUSTED"));
        assert!(!detail.contains("private"));
    }

    #[test]
    fn thought_and_function_calls_are_structured() {
        let event = SseEvent {
            event: None,
            data: r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"plan"},{"functionCall":{"id":"call-1","name":"search","args":{"q":"rust"}},"thoughtSignature":"signature-1"}]},"finishReason":"STOP"}]}"#
                .to_owned(),
        };
        let mut state = GeminiStreamState::default();
        let mut emitted = Vec::new();
        state.handle(event, |event| emitted.push(event)).unwrap();
        assert!(matches!(
            &emitted[0],
            ModelStreamEvent::ReasoningDelta { text } if text == "plan"
        ));
        assert_eq!(emitted.len(), 2);
        assert!(matches!(
            &emitted[1],
            ModelStreamEvent::ToolCallDelta {
                round: 0,
                index: 0,
                call_id,
                name,
                arguments,
                replace: true,
            } if call_id == "call-1" && name == "search" && arguments == r#"{"q":"rust"}"#
        ));
        let completion = state.finish().unwrap();
        assert_eq!(completion.stop_reason, StopReason::Stop);
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].id, "call-1");
        assert_eq!(completion.tool_calls[0].name, "search");
        assert_eq!(completion.tool_calls[0].arguments["q"], "rust");
        assert_eq!(
            completion.provider_state.as_ref().unwrap()["parts"][1]["thoughtSignature"],
            "signature-1"
        );
    }

    #[test]
    fn signature_only_stream_part_gets_valid_data_before_tool_replay() {
        let event = SseEvent {
            event: None,
            data: r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"plan"},{"functionCall":{"name":"search","args":{"q":"rust"}}},{"thoughtSignature":"signature-only"}]},"finishReason":"STOP"}]}"#
                .to_owned(),
        };
        let mut state = GeminiStreamState::default();
        state.handle(event, |_| {}).unwrap();

        let completion = state.finish().unwrap();
        let provider_state = completion.provider_state.unwrap();
        let parts = provider_state["parts"].as_array().unwrap();

        assert_eq!(parts[2]["thoughtSignature"], "signature-only");
        assert_eq!(parts[2]["text"], "");
        assert!(
            parts
                .iter()
                .all(|part| { part.as_object().is_some_and(gemini_part_has_data) })
        );
    }

    #[test]
    fn generate_content_encodes_only_conversation_text() {
        let user = Message::user("hello".to_owned(), None);
        let contents = gemini_contents(&[user]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn gemini_encodes_function_declarations_calls_and_responses() {
        let tools = vec![ToolDefinition {
            model_name: "mcp_exa__search".to_owned(),
            server_name: "exa".to_owned(),
            remote_name: "search".to_owned(),
            description: Some("Search the web".to_owned()),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        assert_eq!(gemini_tools(&tools)[0]["name"], "mcp_exa__search");

        let mut contents = Vec::new();
        append_gemini_tool_rounds(
            &mut contents,
            &[ToolRound {
                calls: vec![ToolCall {
                    index: 0,
                    id: "call-1".to_owned(),
                    provider_id: Some("call-1".to_owned()),
                    name: "mcp_exa__search".to_owned(),
                    arguments: serde_json::json!({"q":"weather"}),
                }],
                results: vec![crate::runtime::tool::ToolResult {
                    call_id: "call-1".to_owned(),
                    name: "mcp_exa__search".to_owned(),
                    output: serde_json::json!({"temperature":24}),
                    is_error: false,
                }],
                provider_state: None,
            }],
        );
        assert_eq!(contents[0]["role"], "model");
        assert_eq!(contents[0]["parts"][0]["functionCall"]["id"], "call-1");
        assert_eq!(contents[1]["role"], "user");
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["response"]["output"]["temperature"],
            24
        );

        let mut no_id_contents = Vec::new();
        append_gemini_tool_rounds(
            &mut no_id_contents,
            &[ToolRound {
                calls: vec![ToolCall {
                    index: 0,
                    id: "internal-1".to_owned(),
                    provider_id: None,
                    name: "mcp_exa__search".to_owned(),
                    arguments: serde_json::json!({}),
                }],
                results: vec![crate::runtime::tool::ToolResult {
                    call_id: "internal-1".to_owned(),
                    name: "mcp_exa__search".to_owned(),
                    output: serde_json::json!({}),
                    is_error: false,
                }],
                provider_state: None,
            }],
        );
        assert!(
            no_id_contents[0]["parts"][0]["functionCall"]
                .get("id")
                .is_none()
        );
        assert!(
            no_id_contents[1]["parts"][0]["functionResponse"]
                .get("id")
                .is_none()
        );
    }
}
