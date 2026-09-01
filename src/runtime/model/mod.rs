//! 统一 Model Runtime：Provider Adapter、能力校验、流事件、usage 与模型目录。

mod anthropic;
mod gemini;
mod openai;
mod transport;

use color_eyre::eyre::{Report, Result, bail};
use serde_json::{Map, Value};
use std::{
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc;

use crate::{
    config::{
        ApiKind, CapabilitySupport, ContextSettings, ModelCapabilities, ModelSettings, Provider,
        ProxySettings, ReasoningSettings,
    },
    event::{AppEvent, Event},
    i18n::Lang,
    runtime::{
        context::{ContextBudget, ContextInputs, build_budget, estimate_tokens},
        error::{
            CancellationError, ContextOverflowError, NetworkStage, NetworkTimeoutError,
            RuntimeError,
        },
        mcp::McpRegistry,
        task::{CancellationReason, CancellationToken},
        tool::{ToolCall, ToolDefinition, ToolResult, ToolRound},
    },
    secret::SecretRedactor,
    session::Message,
};

#[cfg(test)]
pub(crate) use transport::SseDecoder;
pub(crate) use transport::SseEvent;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

const TITLE_TIMEOUT_SECONDS: u64 = 60;
const MODEL_FETCH_TIMEOUT_SECONDS: u64 = 60;
const MAX_TOOL_ROUNDS: usize = 8;
const MAX_TOOL_CALLS_PER_ROUND: usize = 16;
const MAX_MODEL_TOOLS: usize = 128;

/// 一次响应的 token 用量（接口未返回时为 `None`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Stop,
    StopSequence,
    MaxOutputTokens,
    ContentFilter,
    Refusal,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub usage: Option<Usage>,
    pub stop_reason: StopReason,
    pub tool_calls: Vec<ToolCall>,
    pub provider_state: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl RequestOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestMetrics {
    pub request_id: Option<String>,
    pub total_ms: u64,
    pub ttft_ms: Option<u64>,
    pub output_tokens: Option<u32>,
    pub outcome: RequestOutcome,
    pub cancellation_reason: Option<CancellationReason>,
}

/// Provider Adapter 统一输出的流事件。Provider 特有 payload 必须先映射到此契约，
/// 不能把 reasoning 或系统状态拼进助手正文。
#[derive(Debug, Clone, PartialEq)]
pub enum ModelStreamEvent {
    AssistantTextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    System {
        message: String,
    },
    ToolCall {
        call_id: String,
        server: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        server: String,
        name: String,
        output: Value,
        is_error: bool,
    },
    Completed {
        usage: Option<Usage>,
        stop_reason: StopReason,
        metrics: RequestMetrics,
    },
    Error {
        error: RuntimeError,
        metrics: RequestMetrics,
    },
    Cancelled {
        reason: CancellationReason,
        metrics: RequestMetrics,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub capabilities: ModelCapabilities,
}

pub type ModelCatalog = Vec<ModelInfo>;

/// 当前请求实际需要的能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestFeatures {
    pub text: bool,
    pub vision: bool,
    pub audio: bool,
    pub reasoning: bool,
    pub structured_output: bool,
    pub streaming: bool,
}

impl RequestFeatures {
    pub const fn text_stream() -> Self {
        Self {
            text: true,
            vision: false,
            audio: false,
            reasoning: false,
            structured_output: false,
            streaming: true,
        }
    }
}

impl Default for RequestFeatures {
    fn default() -> Self {
        Self::text_stream()
    }
}

#[derive(Clone)]
pub(crate) struct StreamSink {
    stream_id: u64,
    sender: mpsc::UnboundedSender<Event>,
    started: Instant,
    metrics: Arc<Mutex<MetricsState>>,
}

impl StreamSink {
    pub fn record_request_id(&self, request_id: Option<String>) {
        if let Some(request_id) = request_id {
            self.with_metrics(|metrics| {
                if metrics.request_id.is_none() {
                    metrics.request_id = Some(request_id);
                }
            });
        }
    }

    fn emit(&self, event: ModelStreamEvent) {
        if matches!(
            &event,
            ModelStreamEvent::AssistantTextDelta { text }
                | ModelStreamEvent::ReasoningDelta { text }
                if text.is_empty()
        ) {
            return;
        }
        self.with_metrics(|metrics| {
            if let ModelStreamEvent::AssistantTextDelta { text } = &event {
                metrics.output_chars = metrics.output_chars.saturating_add(text.chars().count());
            }
            if metrics.ttft_ms.is_none()
                && matches!(
                    &event,
                    ModelStreamEvent::AssistantTextDelta { .. }
                        | ModelStreamEvent::ReasoningDelta { .. }
                )
            {
                metrics.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
            }
        });
        let _ = self.sender.send(Event::App(AppEvent::ModelStream {
            stream: self.stream_id,
            event,
        }));
    }

    fn with_metrics(&self, update: impl FnOnce(&mut MetricsState)) {
        let mut metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut metrics);
    }

    fn snapshot(
        &self,
        total_ms: u64,
        output_tokens: Option<u32>,
        outcome: RequestOutcome,
        cancellation_reason: Option<CancellationReason>,
    ) -> RequestMetrics {
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RequestMetrics {
            request_id: metrics.request_id.clone(),
            total_ms,
            ttft_ms: metrics.ttft_ms,
            output_tokens: output_tokens.or_else(|| {
                (metrics.output_chars > 0)
                    .then(|| metrics.output_chars.div_ceil(4).min(u32::MAX as usize) as u32)
            }),
            outcome,
            cancellation_reason,
        }
    }
}

#[derive(Debug, Default)]
struct MetricsState {
    request_id: Option<String>,
    ttft_ms: Option<u64>,
    output_chars: usize,
}

pub(crate) struct AdapterStreamRequest<'a> {
    pub provider: &'a Provider,
    pub proxy: &'a ProxySettings,
    pub model: &'a str,
    pub settings: &'a ModelSettings,
    pub compacted_context: Option<&'a str>,
    pub history: &'a [Message],
    pub tools: &'a [ToolDefinition],
    pub tool_rounds: &'a [ToolRound],
    pub cancellation: &'a CancellationToken,
}

pub(crate) struct AdapterTitleRequest<'a> {
    pub provider: &'a Provider,
    pub proxy: &'a ProxySettings,
    pub model: &'a str,
    pub settings: &'a ModelSettings,
    pub instructions: &'a str,
    pub dialog: &'a str,
}

/// 所有 Provider 原生协议实现都通过此边界提供流式回复、标题和模型目录。
pub(crate) trait ProviderAdapter: Sync {
    fn stream_reply<'a>(
        &'a self,
        request: AdapterStreamRequest<'a>,
        sink: StreamSink,
    ) -> BoxFuture<'a, Completion>;

    fn generate_title<'a>(&'a self, request: AdapterTitleRequest<'a>) -> BoxFuture<'a, String>;

    fn fetch_models<'a>(
        &'a self,
        provider: &'a Provider,
        proxy: &'a ProxySettings,
    ) -> BoxFuture<'a, ModelCatalog>;
}

fn adapter_for(kind: ApiKind) -> &'static dyn ProviderAdapter {
    match kind {
        ApiKind::ChatCompletions => &openai::CHAT_ADAPTER,
        ApiKind::Responses => &openai::RESPONSES_ADAPTER,
        ApiKind::AnthropicMessages => &anthropic::ANTHROPIC_ADAPTER,
        ApiKind::GeminiGenerateContent => &gemini::GEMINI_ADAPTER,
    }
}

/// 请求前能力和上下文预算校验。`Unknown` 允许尝试，显式 `Unsupported` 拒绝。
pub fn validate_request(
    provider: &Provider,
    model: &str,
    settings: &ModelSettings,
    budget: &ContextBudget,
    mut features: RequestFeatures,
) -> Result<()> {
    if model.trim().is_empty() {
        bail!("model name is empty");
    }
    if settings.parameters.reasoning.is_some() {
        features.reasoning = true;
    }
    let capabilities = &settings.capabilities;
    for (needed, support, name) in [
        (features.text, capabilities.text, "text"),
        (features.vision, capabilities.vision, "vision"),
        (features.audio, capabilities.audio, "audio"),
        (features.reasoning, capabilities.reasoning, "reasoning"),
        (
            features.structured_output,
            capabilities.structured_output,
            "structured output",
        ),
        (features.streaming, capabilities.streaming, "streaming"),
    ] {
        if needed && support == CapabilitySupport::Unsupported {
            bail!("model {model:?} explicitly does not support {name}");
        }
    }

    if let (Some(requested), Some(limit)) = (
        settings.parameters.max_output_tokens,
        capabilities.max_output_tokens,
    ) && requested > limit
    {
        bail!("model {model:?} max_output_tokens is {limit}, but the request asks for {requested}");
    }
    if budget.overflowed() {
        let context_window = budget.context_window.unwrap_or_default();
        return Err(Report::new(ContextOverflowError {
            message: format!(
                "estimated input {} + reserved output {} exceeds model {model:?} context window {context_window} by {} tokens",
                budget.input_tokens, budget.reserved_output, budget.overflow_tokens
            ),
        }));
    }
    validate_reasoning_kind(provider.api_kind, settings.parameters.reasoning.as_ref())?;
    Ok(())
}

fn validate_reasoning_kind(kind: ApiKind, reasoning: Option<&ReasoningSettings>) -> Result<()> {
    match reasoning {
        Some(ReasoningSettings::OpenAi { .. })
            if !matches!(kind, ApiKind::ChatCompletions | ApiKind::Responses) =>
        {
            bail!("OpenAI reasoning settings cannot be sent to {kind:?}")
        }
        Some(ReasoningSettings::Anthropic { .. }) if kind != ApiKind::AnthropicMessages => {
            bail!("Anthropic thinking settings cannot be sent to {kind:?}")
        }
        Some(ReasoningSettings::Gemini { .. }) if kind != ApiKind::GeminiGenerateContent => {
            bail!("Gemini thinking settings cannot be sent to {kind:?}")
        }
        _ => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_stream_reply(
    provider: Provider,
    proxy: ProxySettings,
    model: String,
    model_settings: ModelSettings,
    context_settings: ContextSettings,
    compacted_context: Option<String>,
    history: Vec<Message>,
    mcp_registry: McpRegistry,
    stream_id: u64,
    cancellation: CancellationToken,
    sender: mpsc::UnboundedSender<Event>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let started = Instant::now();
        let sink = StreamSink {
            stream_id,
            sender: sender.clone(),
            started,
            metrics: Arc::new(Mutex::new(MetricsState::default())),
        };
        tracing::info!(
            stream = stream_id,
            provider_id = ?provider.id,
            provider = ?provider.name,
            api_kind = ?provider.api_kind,
            proxy_mode = ?proxy.mode,
            model = ?model,
            "开始流式回复"
        );
        let operation = async {
            let compacted_messages = compacted_context
                .as_ref()
                .map(|context| Message::user(context.clone(), None));
            let mut budget_history =
                Vec::with_capacity(history.len() + usize::from(compacted_messages.is_some()));
            budget_history.extend(compacted_messages);
            budget_history.extend(history.iter().cloned());
            let mut budget = build_budget(
                &model_settings,
                &context_settings,
                ContextInputs {
                    system_instructions: &[],
                    summary: None,
                    messages: &budget_history,
                },
            );
            let tools = if matches!(
                provider.api_kind,
                ApiKind::ChatCompletions | ApiKind::Responses | ApiKind::GeminiGenerateContent
            ) {
                mcp_registry.tools()
            } else {
                Vec::new()
            };
            if tools.len() > MAX_MODEL_TOOLS {
                bail!(
                    "{} MCP tools are connected; the per-request limit is {MAX_MODEL_TOOLS}",
                    tools.len()
                );
            }
            let mut tool_names = BTreeSet::new();
            if tools
                .iter()
                .any(|tool| !tool_names.insert(tool.model_name.clone()))
            {
                bail!("connected MCP tools produced a model-name collision");
            }
            budget.add_system_tokens(estimated_tool_catalog_tokens(&tools));
            let policy = transport::RequestPolicy::from_parameters(&model_settings.parameters);
            let mut rounds = Vec::new();
            let mut total_usage = None;
            tokio::time::timeout(
                policy.overall_timeout,
                async {
                    for round_index in 0..=MAX_TOOL_ROUNDS {
                        if cancellation.is_cancelled() {
                            return Err(Report::new(CancellationError));
                        }
                        let mut round_budget = budget.clone();
                        round_budget
                            .add_conversation_tokens(estimated_tool_round_tokens(&rounds));
                        validate_request(
                            &provider,
                            &model,
                            &model_settings,
                            &round_budget,
                            RequestFeatures::text_stream(),
                        )?;
                        let request = AdapterStreamRequest {
                            provider: &provider,
                            proxy: &proxy,
                            model: &model,
                            settings: &model_settings,
                            compacted_context: compacted_context.as_deref(),
                            history: &history,
                            tools: &tools,
                            tool_rounds: &rounds,
                            cancellation: &cancellation,
                        };
                        let mut completion = adapter_for(provider.api_kind)
                            .stream_reply(request, sink.clone())
                            .await?;
                        merge_usage(&mut total_usage, completion.usage);
                        if completion.tool_calls.is_empty() {
                            completion.usage = total_usage;
                            return Ok(completion);
                        }
                        if round_index == MAX_TOOL_ROUNDS {
                            bail!("model exceeded the limit of {MAX_TOOL_ROUNDS} MCP tool rounds");
                        }
                        if completion.tool_calls.len() > MAX_TOOL_CALLS_PER_ROUND {
                            bail!(
                                "model requested {} tools in one round; the limit is {MAX_TOOL_CALLS_PER_ROUND}",
                                completion.tool_calls.len()
                            );
                        }
                        let mut call_ids = BTreeSet::new();
                        if completion.tool_calls.iter().any(|call| {
                            call.id.trim().is_empty() || !call_ids.insert(call.id.clone())
                        }) {
                            bail!("model returned empty or duplicate tool call ids");
                        }
                        let calls = completion.tool_calls;
                        let provider_state = completion.provider_state;
                        let mut results = Vec::with_capacity(calls.len());
                        for call in &calls {
                            let definition = tools
                                .iter()
                                .find(|tool| tool.model_name == call.name);
                            let server = definition
                                .map(|tool| tool.server_name.clone())
                                .unwrap_or_else(|| "unavailable".to_owned());
                            let remote_name = definition
                                .map(|tool| tool.remote_name.as_str())
                                .unwrap_or(&call.name);
                            let remote_name = display_tool_name(remote_name);
                            sink.emit(ModelStreamEvent::ToolCall {
                                call_id: call.id.clone(),
                                server: server.clone(),
                                name: remote_name.clone(),
                                arguments: call.arguments.clone(),
                            });
                            let result = match mcp_registry.call_tool(call, &cancellation).await {
                                Ok(result) => result,
                                Err(_) if cancellation.is_cancelled() => {
                                    return Err(Report::new(CancellationError));
                                }
                                Err(error) => ToolResult {
                                    call_id: call.id.clone(),
                                    name: call.name.clone(),
                                    output: serde_json::json!({ "error": error }),
                                    is_error: true,
                                },
                            };
                            sink.emit(ModelStreamEvent::ToolResult {
                                call_id: result.call_id.clone(),
                                server,
                                name: remote_name,
                                output: result.output.clone(),
                                is_error: result.is_error,
                            });
                            results.push(result);
                        }
                        rounds.push(ToolRound {
                            calls,
                            results,
                            provider_state,
                        });
                    }
                    unreachable!("bounded MCP tool loop always returns")
                },
            )
            .await
            .map_err(|_| {
                Report::new(NetworkTimeoutError {
                    stage: NetworkStage::Overall,
                    seconds: policy.overall_timeout.as_secs(),
                })
            })?
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(Report::new(CancellationError)),
            result = operation => result,
        };

        let elapsed_ms = started.elapsed().as_millis() as u64;
        let event = match result {
            Ok(completion) => {
                let usage = completion.usage;
                let metrics = sink.snapshot(
                    elapsed_ms,
                    usage.map(|usage| usage.completion_tokens),
                    RequestOutcome::Completed,
                    None,
                );
                let (prompt_tokens, completion_tokens, total_tokens) = usage
                    .map(|usage| {
                        (
                            usage.prompt_tokens,
                            usage.completion_tokens,
                            usage.total_tokens,
                        )
                    })
                    .unwrap_or_default();
                tracing::info!(
                    stream = stream_id,
                    provider_id = ?provider.id,
                    provider = ?provider.name,
                    api_kind = ?provider.api_kind,
                    model = ?model,
                    request_id = ?metrics.request_id,
                    total_ms = metrics.total_ms,
                    ttft_ms = ?metrics.ttft_ms,
                    stop_reason = ?completion.stop_reason,
                    has_usage = usage.is_some(),
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    output_tokens = ?metrics.output_tokens,
                    completion_status = metrics.outcome.as_str(),
                    "流式回复完成"
                );
                ModelStreamEvent::Completed {
                    usage,
                    stop_reason: completion.stop_reason,
                    metrics,
                }
            }
            Err(err) => {
                let cancelled = cancellation.is_cancelled()
                    || err
                        .chain()
                        .any(|cause| cause.downcast_ref::<CancellationError>().is_some());
                if cancelled {
                    let reason = cancellation.reason();
                    let metrics =
                        sink.snapshot(elapsed_ms, None, RequestOutcome::Cancelled, Some(reason));
                    tracing::info!(
                        stream = stream_id,
                        provider_id = ?provider.id,
                        provider = ?provider.name,
                        api_kind = ?provider.api_kind,
                        model = ?model,
                        request_id = ?metrics.request_id,
                        total_ms = metrics.total_ms,
                        ttft_ms = ?metrics.ttft_ms,
                        output_tokens = ?metrics.output_tokens,
                        completion_status = metrics.outcome.as_str(),
                        cancel_reason = metrics
                            .cancellation_reason
                            .unwrap_or(reason)
                            .as_str(),
                        "流式回复已取消"
                    );
                    ModelStreamEvent::Cancelled { reason, metrics }
                } else {
                    let detail = error_for_log(&err, &provider, &proxy);
                    let runtime_error = RuntimeError::from_report(&err, detail);
                    sink.record_request_id(runtime_error.request_id.clone());
                    let metrics = sink.snapshot(elapsed_ms, None, RequestOutcome::Failed, None);
                    tracing::error!(
                        stream = stream_id,
                        provider_id = ?provider.id,
                        provider = ?provider.name,
                        api_kind = ?provider.api_kind,
                        model = ?model,
                        request_id = ?metrics.request_id,
                        total_ms = metrics.total_ms,
                        ttft_ms = ?metrics.ttft_ms,
                        output_tokens = ?metrics.output_tokens,
                        completion_status = metrics.outcome.as_str(),
                        error_kind = runtime_error.kind.as_str(),
                        error = %runtime_error.detail,
                        "流式回复失败"
                    );
                    ModelStreamEvent::Error {
                        error: runtime_error,
                        metrics,
                    }
                }
            }
        };
        let _ = sender.send(Event::App(AppEvent::ModelStream {
            stream: stream_id,
            event,
        }));
    })
}

fn merge_usage(total: &mut Option<Usage>, current: Option<Usage>) {
    let Some(current) = current else {
        return;
    };
    let aggregate = total.get_or_insert_default();
    aggregate.prompt_tokens = aggregate
        .prompt_tokens
        .saturating_add(current.prompt_tokens);
    aggregate.completion_tokens = aggregate
        .completion_tokens
        .saturating_add(current.completion_tokens);
    aggregate.total_tokens = aggregate.total_tokens.saturating_add(current.total_tokens);
}

fn display_tool_name(name: &str) -> String {
    let mut display = name
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect::<String>();
    if display.is_empty() {
        display.push_str("tool");
    }
    display
}

fn estimated_tool_catalog_tokens(tools: &[ToolDefinition]) -> usize {
    tools.iter().fold(0, |total, tool| {
        let description = tool.description.as_deref().unwrap_or_default();
        let schema = tool.input_schema.to_string();
        total
            .saturating_add(8)
            .saturating_add(estimate_tokens(&tool.model_name))
            .saturating_add(estimate_tokens(description))
            .saturating_add(estimate_tokens(&schema))
    })
}

fn estimated_tool_round_tokens(rounds: &[ToolRound]) -> usize {
    rounds.iter().fold(0, |total, round| {
        let calls = round.calls.iter().fold(0usize, |subtotal, call| {
            subtotal
                .saturating_add(8)
                .saturating_add(estimate_tokens(&call.id))
                .saturating_add(estimate_tokens(&call.name))
                .saturating_add(estimate_tokens(&call.arguments.to_string()))
        });
        let results = round.results.iter().fold(0usize, |subtotal, result| {
            subtotal
                .saturating_add(8)
                .saturating_add(estimate_tokens(&result.call_id))
                .saturating_add(estimate_tokens(&result.name))
                .saturating_add(estimate_tokens(&result.output.to_string()))
        });
        let provider_state = round
            .provider_state
            .as_ref()
            .map_or(0, |state| estimate_tokens(&state.to_string()));
        total
            .saturating_add(calls)
            .saturating_add(results)
            .saturating_add(provider_state)
    })
}

pub struct TitleGenerationRequest {
    pub provider: Provider,
    pub proxy: ProxySettings,
    pub model: String,
    pub lang: Lang,
    pub session_id: String,
    pub first_user: String,
    pub first_reply: String,
}

pub fn spawn_generate_title(
    request: TitleGenerationRequest,
    sender: mpsc::UnboundedSender<Event>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let TitleGenerationRequest {
            provider,
            proxy,
            model,
            lang,
            session_id,
            first_user,
            first_reply,
        } = request;
        let texts = lang.texts();
        let (user_role, assistant_role) = dialog_roles(lang);
        let dialog = format!(
            "{user_role}: {}\n{assistant_role}: {}",
            excerpt(&first_user, 600, texts.llm_empty),
            excerpt(&first_reply, 600, texts.llm_empty)
        );
        let result = async {
            let settings = provider.settings_for_model(&model);
            let system_layers = [texts.title_system_prompt, dialog.as_str()];
            let budget = build_budget(
                &settings,
                &ContextSettings::default(),
                ContextInputs {
                    system_instructions: &system_layers,
                    summary: None,
                    messages: &[],
                },
            );
            validate_request(
                &provider,
                &model,
                &settings,
                &budget,
                RequestFeatures {
                    streaming: false,
                    ..RequestFeatures::text_stream()
                },
            )?;
            let request = AdapterTitleRequest {
                provider: &provider,
                proxy: &proxy,
                model: &model,
                settings: &settings,
                instructions: texts.title_system_prompt,
                dialog: &dialog,
            };
            adapter_for(provider.api_kind).generate_title(request).await
        };
        match tokio::time::timeout(
            std::time::Duration::from_secs(TITLE_TIMEOUT_SECONDS),
            result,
        )
        .await
        {
            Ok(Ok(raw)) => {
                let title = clean_title(&raw);
                if title.is_empty() {
                    tracing::warn!(session = ?session_id, "会话标题响应为空");
                    return;
                }
                tracing::info!(
                    session = ?session_id,
                    provider = ?provider.name,
                    model = ?model,
                    "会话标题生成完成"
                );
                let _ = sender.send(Event::App(AppEvent::TitleGenerated {
                    session: session_id,
                    title,
                }));
            }
            Ok(Err(err)) => tracing::warn!(
                session = ?session_id,
                provider = ?provider.name,
                api_kind = ?provider.api_kind,
                model = ?model,
                error = %error_for_log(&err, &provider, &proxy),
                "会话标题生成失败"
            ),
            Err(_) => tracing::warn!(
                session = ?session_id,
                provider = ?provider.name,
                model = ?model,
                timeout_secs = TITLE_TIMEOUT_SECONDS,
                "会话标题生成超时"
            ),
        }
    })
}

pub fn spawn_fetch_models(
    provider: Provider,
    proxy: ProxySettings,
    task_id: u64,
    sender: mpsc::UnboundedSender<Event>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(MODEL_FETCH_TIMEOUT_SECONDS),
            adapter_for(provider.api_kind).fetch_models(&provider, &proxy),
        )
        .await
        .map_err(|_| {
            color_eyre::eyre::eyre!(
                "model list request timed out after {MODEL_FETCH_TIMEOUT_SECONDS} seconds"
            )
        })
        .and_then(|result| result);
        let models = match result {
            Ok(mut catalog) => {
                catalog.retain(|model| !model.id.trim().is_empty());
                catalog.sort_by(|left, right| left.id.cmp(&right.id));
                catalog.dedup_by(|left, right| left.id == right.id);
                tracing::info!(
                    task = task_id,
                    provider = ?provider.name,
                    models = catalog.len(),
                    "模型列表同步完成"
                );
                Ok(catalog)
            }
            Err(err) => {
                let detail = error_for_log(&err, &provider, &proxy);
                tracing::warn!(
                    task = task_id,
                    provider = ?provider.name,
                    api_kind = ?provider.api_kind,
                    error = %detail,
                    "模型列表同步失败"
                );
                Err(detail)
            }
        };
        let _ = sender.send(Event::App(AppEvent::ModelsSynced {
            task: task_id,
            models,
        }));
    })
}

pub(crate) fn apply_extra_body(
    body: &mut Value,
    extra: &std::collections::BTreeMap<String, Value>,
) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    for (key, value) in extra {
        object.insert(key.clone(), value.clone());
    }
}

pub(crate) fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>> {
    value
        .as_object_mut()
        .ok_or_else(|| color_eyre::eyre::eyre!("request JSON must be an object"))
}

fn error_for_log(err: &Report, provider: &Provider, proxy: &ProxySettings) -> String {
    let detail = err
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ");
    let mut redactor = SecretRedactor::new();
    if let Some(api_key) = provider.api_key.as_ref() {
        redactor.add_configured(api_key.expose());
    }
    for (name, value) in &provider.headers {
        redactor.add_header(name, value);
    }
    if let Some(raw_url) = provider.base_url.as_deref() {
        redactor.add_url(raw_url, false);
    }
    if let Some(raw_url) = proxy.url.as_deref() {
        redactor.add_url(raw_url, false);
    }
    redactor.redact(&detail).chars().take(2_000).collect()
}

fn dialog_roles(lang: Lang) -> (&'static str, &'static str) {
    match lang {
        Lang::Zh => ("用户", "助手"),
        Lang::En => ("User", "Assistant"),
    }
}

fn excerpt(text: &str, max_chars: usize, empty_label: &str) -> String {
    let mut out: String = text.trim().chars().take(max_chars).collect();
    if out.is_empty() {
        out.push_str(empty_label);
    }
    out
}

fn clean_title(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | '`' | '“' | '”' | '『' | '』' | '「' | '」'
            )
        })
        .trim();
    let single_line = unquoted.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line.chars().take(24).collect()
}

pub(crate) fn u32_token(value: Option<u64>) -> u32 {
    value.unwrap_or_default().min(u32::MAX as u64) as u32
}

pub(crate) fn diagnostic_label(value: &str) -> String {
    value.chars().take(128).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CapabilitySupport, ModelParameters};

    #[test]
    fn request_validation_rejects_only_explicitly_unsupported_features() {
        let mut provider = crate::config::Settings::default().provider().clone();
        let mut settings = ModelSettings::default();
        settings.capabilities.vision = CapabilitySupport::Unsupported;
        provider
            .model_settings
            .insert("gpt-4o-mini".to_owned(), settings.clone());
        let budget = build_budget(
            &settings,
            &ContextSettings::default(),
            ContextInputs::conversation(None, &[]),
        );

        assert!(
            validate_request(
                &provider,
                "gpt-4o-mini",
                &settings,
                &budget,
                RequestFeatures::text_stream()
            )
            .is_ok()
        );
        let mut features = RequestFeatures::text_stream();
        features.vision = true;
        assert!(validate_request(&provider, "gpt-4o-mini", &settings, &budget, features).is_err());
    }

    #[test]
    fn context_and_output_limits_are_checked_locally() {
        let mut provider = crate::config::Settings::default().provider().clone();
        let settings = ModelSettings {
            capabilities: ModelCapabilities {
                context_window: Some(100),
                max_output_tokens: Some(20),
                ..ModelCapabilities::default()
            },
            parameters: ModelParameters {
                max_output_tokens: Some(30),
                ..ModelParameters::default()
            },
        };
        provider
            .model_settings
            .insert("gpt-4o-mini".to_owned(), settings.clone());
        let budget = build_budget(
            &settings,
            &ContextSettings::default(),
            ContextInputs::conversation(None, &[]),
        );
        assert!(
            validate_request(
                &provider,
                "gpt-4o-mini",
                &settings,
                &budget,
                RequestFeatures::text_stream()
            )
            .is_err()
        );
    }

    #[test]
    fn log_redaction_covers_custom_headers_and_url_credentials() {
        let mut provider = crate::config::Settings::default().provider().clone();
        provider.api_key = Some(crate::secret::SecretValue::from("secret-key"));
        provider.base_url = Some("https://user:password@example.com/v1".to_owned());
        provider
            .headers
            .insert("cookie".to_owned(), "session=private".to_owned());
        let proxy = ProxySettings {
            mode: crate::config::ProxyMode::Http,
            url: Some("http://proxy:proxy-secret@localhost:7890".to_owned()),
        };
        let report =
            color_eyre::eyre::eyre!("secret-key session=private user password proxy-secret");
        let detail = error_for_log(&report, &provider, &proxy);
        assert!(!detail.contains("secret-key"));
        assert!(!detail.contains("session=private"));
        assert!(!detail.contains("proxy-secret"));
    }

    #[test]
    fn title_cleanup_is_bounded_and_single_line() {
        assert_eq!(clean_title("  「你好\n世界」 "), "你好 世界");
        assert!(clean_title(&"很长".repeat(30)).chars().count() <= 24);
    }

    #[test]
    fn stream_metrics_capture_request_id_ttft_and_partial_output() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let sink = StreamSink {
            stream_id: 7,
            sender,
            started: Instant::now(),
            metrics: Arc::new(Mutex::new(MetricsState::default())),
        };
        sink.record_request_id(Some("req-test".to_owned()));
        sink.emit(ModelStreamEvent::AssistantTextDelta {
            text: "12345".to_owned(),
        });
        let metrics = sink.snapshot(20, None, RequestOutcome::Failed, None);
        assert_eq!(metrics.request_id.as_deref(), Some("req-test"));
        assert!(metrics.ttft_ms.is_some());
        assert_eq!(metrics.output_tokens, Some(2));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Event::App(AppEvent::ModelStream { stream: 7, .. })
        ));
    }
}
