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
    let mut body = serde_json::json!({ "contents": contents });
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
        .filter(|message| message.role == Role::User || !message.content.is_empty())
        .map(|message| {
            serde_json::json!({
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "model",
                },
                "parts": [{"text": message.content}],
            })
        })
        .collect()
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
        Ok(Completion {
            usage: self.usage,
            stop_reason,
        })
    }
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
    fn thought_is_structured_and_non_chat_parts_are_ignored() {
        let event = SseEvent {
            event: None,
            data: r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"plan"},{"functionCall":{"id":"call-1","name":"search","args":{"q":"rust"}}}]},"finishReason":"STOP"}]}"#
                .to_owned(),
        };
        let mut state = GeminiStreamState::default();
        let mut emitted = Vec::new();
        state.handle(event, |event| emitted.push(event)).unwrap();
        assert!(matches!(
            &emitted[0],
            ModelStreamEvent::ReasoningDelta { text } if text == "plan"
        ));
        assert_eq!(emitted.len(), 1);
        assert_eq!(state.finish().unwrap().stop_reason, StopReason::Stop);
    }

    #[test]
    fn generate_content_encodes_only_conversation_text() {
        let user = Message::user("hello".to_owned(), None);
        let contents = gemini_contents(&[user]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
    }
}
