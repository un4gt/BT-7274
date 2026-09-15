//! 远程 MCP 连接管理。
//!
//! 这里只支持 Streamable HTTP；服务端可用 JSON 或 SSE 返回流式响应。
//! Runtime 负责初始化、工具发现、工具调用、状态监测、重连和关闭。

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, JsonObject},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    config::{McpCapabilityMetadata, McpServerConfig, ProxySettings},
    event::{AppEvent, Event},
    runtime::task::CancellationToken,
    runtime::tool::{ToolCall, ToolDefinition, ToolResult},
    secret::{SecretRedactor, resolve_env_variable},
};

const MCP_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
const MCP_TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(60);
const MCP_TOOL_RESULT_MAX_BYTES: usize = 256 * 1024;
const MCP_TOOL_SCHEMA_MAX_BYTES: usize = 64 * 1024;
const MCP_TOOLS_PER_SERVER_MAX: usize = 128;

type ClientService = RunningService<RoleClient, ()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    Disabled,
    Starting,
    Connected,
    Failed,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSnapshot {
    pub name: String,
    pub status: McpServerStatus,
    pub generation: u64,
    pub capabilities: McpCapabilityMetadata,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRuntimeEvent {
    Changed {
        server_name: String,
        generation: u64,
    },
}

#[derive(Clone)]
pub struct McpRegistry {
    inner: Arc<McpRegistryInner>,
}

struct McpRegistryInner {
    state: Mutex<RegistryState>,
    events: mpsc::UnboundedSender<Event>,
}

#[derive(Default)]
struct RegistryState {
    next_generation: u64,
    shutting_down: bool,
    servers: BTreeMap<String, ServerEntry>,
    retired: Vec<JoinHandle<()>>,
}

struct ServerEntry {
    snapshot: McpServerSnapshot,
    tools: Vec<ToolDefinition>,
    commands: Option<mpsc::UnboundedSender<McpCommand>>,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
}

enum McpCommand {
    Call {
        remote_name: String,
        arguments: JsonObject,
        cancellation: CancellationToken,
        response: oneshot::Sender<Result<CallToolResult, String>>,
    },
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl fmt::Debug for McpRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock_state();
        formatter
            .debug_struct("McpRegistry")
            .field("servers", &state.servers.len())
            .field("shutting_down", &state.shutting_down)
            .finish()
    }
}

impl Drop for McpRegistryInner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for entry in state.servers.values() {
            entry.shutdown.cancel();
        }
    }
}

impl McpRegistry {
    pub fn new(events: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            inner: Arc::new(McpRegistryInner {
                state: Mutex::new(RegistryState::default()),
                events,
            }),
        }
    }

    pub fn reconfigure(&self, configs: &[McpServerConfig], proxy: &ProxySettings) {
        let mut launches = Vec::new();
        let mut changed = Vec::new();
        {
            let mut state = self.lock_state();
            state.shutting_down = false;
            retire_all(&mut state);
            state.servers.clear();
            state.retired.retain(|task| !task.is_finished());

            for config in configs.iter().cloned() {
                state.next_generation = state.next_generation.wrapping_add(1);
                let generation = state.next_generation;
                let shutdown = CancellationToken::new();
                let (commands, command_receiver) = if config.enabled {
                    let (sender, receiver) = mpsc::unbounded_channel();
                    (Some(sender), Some(receiver))
                } else {
                    (None, None)
                };
                state.servers.insert(
                    config.name.clone(),
                    ServerEntry {
                        snapshot: McpServerSnapshot {
                            name: config.name.clone(),
                            status: if config.enabled {
                                McpServerStatus::Starting
                            } else {
                                McpServerStatus::Disabled
                            },
                            generation,
                            capabilities: McpCapabilityMetadata::default(),
                            error: None,
                        },
                        tools: Vec::new(),
                        commands,
                        shutdown: shutdown.clone(),
                        task: None,
                    },
                );
                changed.push((config.name.clone(), generation));
                if let Some(command_receiver) = command_receiver {
                    launches.push((
                        config,
                        proxy.clone(),
                        generation,
                        shutdown,
                        command_receiver,
                    ));
                }
            }
        }
        for (config, proxy, generation, shutdown, commands) in launches {
            self.spawn_actor(config, proxy, generation, shutdown, commands);
        }
        for (server_name, generation) in changed {
            self.emit_changed(server_name, generation);
        }
    }

    pub fn reconnect(&self, config: McpServerConfig, proxy: ProxySettings) -> Result<(), String> {
        if !config.enabled {
            return Err(format!("MCP Server {:?} is disabled", config.name));
        }
        config.validate().map_err(|error| format!("{error:#}"))?;
        let (generation, shutdown, command_receiver) = {
            let mut state = self.lock_state();
            if state.shutting_down {
                return Err("MCP Registry is shutting down".to_owned());
            }
            if let Some(mut old) = state.servers.remove(&config.name) {
                old.shutdown.cancel();
                if let Some(task) = old.task.take() {
                    state.retired.push(task);
                }
            }
            state.next_generation = state.next_generation.wrapping_add(1);
            let generation = state.next_generation;
            let shutdown = CancellationToken::new();
            let (commands, command_receiver) = mpsc::unbounded_channel();
            state.servers.insert(
                config.name.clone(),
                ServerEntry {
                    snapshot: McpServerSnapshot {
                        name: config.name.clone(),
                        status: McpServerStatus::Starting,
                        generation,
                        capabilities: McpCapabilityMetadata::default(),
                        error: None,
                    },
                    tools: Vec::new(),
                    commands: Some(commands),
                    shutdown: shutdown.clone(),
                    task: None,
                },
            );
            (generation, shutdown, command_receiver)
        };
        self.spawn_actor(
            config.clone(),
            proxy,
            generation,
            shutdown,
            command_receiver,
        );
        self.emit_changed(config.name, generation);
        Ok(())
    }

    pub fn snapshots(&self) -> BTreeMap<String, McpServerSnapshot> {
        self.lock_state()
            .servers
            .iter()
            .map(|(name, entry)| (name.clone(), entry.snapshot.clone()))
            .collect()
    }

    pub fn snapshot(&self, server_name: &str) -> Option<McpServerSnapshot> {
        self.lock_state()
            .servers
            .get(server_name)
            .map(|entry| entry.snapshot.clone())
    }

    /// 当前所有已连接 Server 的工具目录。
    pub fn tools(&self) -> Vec<ToolDefinition> {
        self.lock_state()
            .servers
            .values()
            .filter(|entry| entry.snapshot.status == McpServerStatus::Connected)
            .flat_map(|entry| entry.tools.iter().cloned())
            .collect()
    }

    /// 将模型侧别名解析回目标 Server，并通过该 Server 的 actor 执行 `tools/call`。
    pub async fn call_tool(
        &self,
        call: &ToolCall,
        cancellation: &CancellationToken,
    ) -> Result<ToolResult, String> {
        let target = {
            let state = self.lock_state();
            state.servers.values().find_map(|entry| {
                if entry.snapshot.status != McpServerStatus::Connected {
                    return None;
                }
                entry
                    .tools
                    .iter()
                    .find(|tool| tool.model_name == call.name)
                    .and_then(|tool| {
                        entry
                            .commands
                            .as_ref()
                            .map(|commands| (commands.clone(), tool.remote_name.clone()))
                    })
            })
        };
        let Some((commands, remote_name)) = target else {
            return Ok(tool_error(
                call,
                format!("unknown or unavailable tool {:?}", call.name),
            ));
        };
        let Some(arguments) = call.arguments.as_object().cloned() else {
            return Ok(tool_error(
                call,
                "tool arguments must be a JSON object".to_owned(),
            ));
        };
        let call_cancellation = CancellationToken::new();
        let _cancel_on_drop = CancelOnDrop(call_cancellation.clone());
        let (response, receiver) = oneshot::channel();
        if commands
            .send(McpCommand::Call {
                remote_name,
                arguments,
                cancellation: call_cancellation,
                response,
            })
            .is_err()
        {
            return Ok(tool_error(
                call,
                "MCP Server connection is unavailable".to_owned(),
            ));
        }
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err("MCP tool call was cancelled".to_owned()),
            result = receiver => result,
        };
        match result {
            Ok(Ok(result)) => Ok(normalize_tool_result(call, result)),
            Ok(Err(error)) => Ok(tool_error(call, error)),
            Err(_) => Ok(tool_error(
                call,
                "MCP Server closed before returning a tool result".to_owned(),
            )),
        }
    }

    pub async fn shutdown(&self) {
        let tasks = {
            let mut state = self.lock_state();
            state.shutting_down = true;
            for entry in state.servers.values_mut() {
                entry.snapshot.status = McpServerStatus::Stopping;
                entry.shutdown.cancel();
            }
            let mut tasks = std::mem::take(&mut state.retired);
            tasks.extend(
                state
                    .servers
                    .values_mut()
                    .filter_map(|entry| entry.task.take()),
            );
            tasks
        };
        let abort_handles = tasks
            .iter()
            .map(JoinHandle::abort_handle)
            .collect::<Vec<_>>();
        let wait = async move {
            for task in tasks {
                let _ = task.await;
            }
        };
        if tokio::time::timeout(MCP_SHUTDOWN_TIMEOUT, wait)
            .await
            .is_err()
        {
            tracing::warn!("MCP Runtime 退出清理超时，强制终止剩余连接任务");
            for handle in abort_handles {
                handle.abort();
            }
        }
    }

    fn spawn_actor(
        &self,
        config: McpServerConfig,
        proxy: ProxySettings,
        generation: u64,
        shutdown: CancellationToken,
        commands: mpsc::UnboundedReceiver<McpCommand>,
    ) {
        let registry = self.clone();
        let server_name = config.name.clone();
        let task = tokio::spawn(async move {
            server_actor(registry, config, proxy, generation, shutdown, commands).await;
        });
        let mut state = self.lock_state();
        if let Some(entry) = state
            .servers
            .get_mut(&server_name)
            .filter(|entry| entry.snapshot.generation == generation)
        {
            entry.task = Some(task);
        } else {
            task.abort();
        }
    }

    fn set_connected(
        &self,
        server_name: &str,
        generation: u64,
        capabilities: McpCapabilityMetadata,
        tools: Vec<ToolDefinition>,
    ) {
        let mut state = self.lock_state();
        let Some(entry) = state
            .servers
            .get_mut(server_name)
            .filter(|entry| entry.snapshot.generation == generation)
        else {
            return;
        };
        entry.snapshot.status = McpServerStatus::Connected;
        entry.snapshot.capabilities = capabilities;
        entry.tools = tools;
        entry.snapshot.error = None;
        drop(state);
        self.emit_changed(server_name.to_owned(), generation);
    }

    fn set_failed(&self, server_name: &str, generation: u64, error: String) {
        let mut state = self.lock_state();
        let Some(entry) = state
            .servers
            .get_mut(server_name)
            .filter(|entry| entry.snapshot.generation == generation)
        else {
            return;
        };
        entry.snapshot.status = McpServerStatus::Failed;
        entry.snapshot.capabilities.tool_count = 0;
        entry.tools.clear();
        entry.snapshot.error = Some(error);
        drop(state);
        self.emit_changed(server_name.to_owned(), generation);
    }

    fn emit_changed(&self, server_name: String, generation: u64) {
        let _ =
            self.inner
                .events
                .send(Event::App(AppEvent::McpRuntime(McpRuntimeEvent::Changed {
                    server_name,
                    generation,
                })));
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn retire_all(state: &mut RegistryState) {
    let mut retired = Vec::new();
    for entry in state.servers.values_mut() {
        entry.shutdown.cancel();
        if let Some(task) = entry.task.take() {
            retired.push(task);
        }
    }
    state.retired.extend(retired);
}

async fn server_actor(
    registry: McpRegistry,
    config: McpServerConfig,
    proxy: ProxySettings,
    generation: u64,
    shutdown: CancellationToken,
    mut commands: mpsc::UnboundedReceiver<McpCommand>,
) {
    tracing::info!(server = %config.name, transport = "streamable_http", "MCP Server 开始连接");
    let mut service = match connect_server(&config, &proxy, &shutdown).await {
        Ok(service) => service,
        Err(error) => {
            let error = redact_runtime_error(&error, &config, &proxy);
            tracing::error!(server = %config.name, error = %error, "MCP Server 连接失败");
            registry.set_failed(&config.name, generation, error);
            return;
        }
    };

    let Some(peer) = service.peer_info() else {
        let error = "MCP initialize response did not include peer metadata".to_owned();
        registry.set_failed(&config.name, generation, error);
        close_service(&mut service).await;
        return;
    };
    let mut capabilities = McpCapabilityMetadata {
        protocol_version: Some(peer.protocol_version.as_str().to_owned()),
        server_name: peer
            .server_info
            .as_ref()
            .map(|implementation| implementation.name.clone()),
        server_version: peer
            .server_info
            .as_ref()
            .map(|implementation| implementation.version.clone()),
        resources: peer.capabilities.resources.is_some(),
        prompts: peer.capabilities.prompts.is_some(),
        tools: peer.capabilities.tools.is_some(),
        tool_count: 0,
    };
    let tools = if capabilities.tools {
        match discover_tools(
            &service,
            &config.name,
            Duration::from_secs(config.startup_timeout_sec),
            &shutdown,
        )
        .await
        {
            Ok(tools) => tools,
            Err(error) => {
                let error = redact_runtime_error(&error, &config, &proxy);
                tracing::error!(server = %config.name, error = %error, "MCP tools/list 失败");
                registry.set_failed(&config.name, generation, error);
                close_service(&mut service).await;
                return;
            }
        }
    } else {
        Vec::new()
    };
    capabilities.tool_count = tools.len();
    capabilities.normalize();
    registry.set_connected(&config.name, generation, capabilities, tools);
    tracing::info!(
        server = %config.name,
        tools = registry
            .snapshot(&config.name)
            .map_or(0, |snapshot| snapshot.capabilities.tool_count),
        "MCP Server 已连接"
    );

    let mut health = tokio::time::interval(Duration::from_millis(250));
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = health.tick() => {
                if service.is_closed() {
                    registry.set_failed(
                        &config.name,
                        generation,
                        format!("MCP Server {:?} transport closed unexpectedly", config.name),
                    );
                    break;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                handle_tool_command(&service, command, &config, &proxy, &shutdown).await;
            }
        }
    }
    close_service(&mut service).await;
    tracing::info!(server = %config.name, "MCP Server 连接已关闭");
}

async fn discover_tools(
    service: &ClientService,
    server_name: &str,
    timeout: Duration,
    shutdown: &CancellationToken,
) -> Result<Vec<ToolDefinition>, String> {
    let request = service.list_all_tools();
    tokio::pin!(request);
    let tools = tokio::select! {
        biased;
        _ = shutdown.cancelled() => return Err("MCP tools/list was cancelled".to_owned()),
        result = tokio::time::timeout(timeout, &mut request) => match result {
            Ok(Ok(tools)) => tools,
            Ok(Err(error)) => return Err(format!("MCP tools/list failed: {error}")),
            Err(_) => return Err(format!("MCP tools/list exceeded {} seconds", timeout.as_secs())),
        },
    };
    if tools.len() > MCP_TOOLS_PER_SERVER_MAX {
        return Err(format!(
            "MCP tools/list returned {} tools; the per-server limit is {MCP_TOOLS_PER_SERVER_MAX}",
            tools.len()
        ));
    }
    let mut catalog = BTreeMap::new();
    for tool in tools {
        let remote_name = tool.name.into_owned();
        if remote_name.trim().is_empty() {
            return Err("MCP tools/list returned an empty tool name".to_owned());
        }
        if remote_name.chars().count() > 256 {
            return Err(
                "MCP tools/list returned a tool name longer than 256 characters".to_owned(),
            );
        }
        let input_schema = Value::Object((*tool.input_schema).clone());
        if serde_json::to_vec(&input_schema).map_or(usize::MAX, |encoded| encoded.len())
            > MCP_TOOL_SCHEMA_MAX_BYTES
        {
            return Err(format!(
                "MCP tool {remote_name:?} input schema exceeded the {MCP_TOOL_SCHEMA_MAX_BYTES} byte limit"
            ));
        }
        let definition = ToolDefinition {
            model_name: model_tool_name(server_name, &remote_name),
            server_name: server_name.to_owned(),
            remote_name,
            description: tool.description.and_then(|description| {
                let description = description.trim();
                (!description.is_empty()).then(|| description.chars().take(4_096).collect())
            }),
            input_schema,
        };
        if catalog
            .insert(definition.model_name.clone(), definition)
            .is_some()
        {
            return Err("MCP tools/list returned duplicate or colliding tool names".to_owned());
        }
    }
    Ok(catalog.into_values().collect())
}

async fn handle_tool_command(
    service: &ClientService,
    command: McpCommand,
    config: &McpServerConfig,
    proxy: &ProxySettings,
    shutdown: &CancellationToken,
) {
    let McpCommand::Call {
        remote_name,
        arguments,
        cancellation,
        response,
    } = command;
    let request =
        service.call_tool(CallToolRequestParams::new(remote_name).with_arguments(arguments));
    tokio::pin!(request);
    let result = tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err("MCP Server is shutting down".to_owned()),
        _ = cancellation.cancelled() => Err("MCP tool call was cancelled".to_owned()),
        result = tokio::time::timeout(MCP_TOOL_CALL_TIMEOUT, &mut request) => match result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(redact_runtime_error(&format!("MCP tools/call failed: {error}"), config, proxy)),
            Err(_) => Err(format!("MCP tools/call exceeded {} seconds", MCP_TOOL_CALL_TIMEOUT.as_secs())),
        },
    };
    let _ = response.send(result);
}

fn model_tool_name(server_name: &str, remote_name: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in server_name
        .bytes()
        .chain(std::iter::once(0))
        .chain(remote_name.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut base = format!(
        "mcp_{}__{}",
        safe_tool_component(server_name),
        safe_tool_component(remote_name)
    );
    let suffix = format!("_{hash:016x}");
    base.truncate(64_usize.saturating_sub(suffix.len()));
    base.push_str(&suffix);
    base
}

fn safe_tool_component(value: &str) -> String {
    let mut output = value
        .chars()
        .take(24)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty() {
        output.push_str("tool");
    }
    output
}

fn normalize_tool_result(call: &ToolCall, result: CallToolResult) -> ToolResult {
    let is_error = result.is_error.unwrap_or(false);
    let output = serde_json::to_value(&result)
        .unwrap_or_else(|_| json!({ "error": "MCP tool result could not be serialized" }));
    let size = serde_json::to_vec(&output).map_or(usize::MAX, |encoded| encoded.len());
    if size > MCP_TOOL_RESULT_MAX_BYTES {
        return tool_error(
            call,
            format!(
                "MCP tool result exceeded the {} byte limit",
                MCP_TOOL_RESULT_MAX_BYTES
            ),
        );
    }
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        output,
        is_error,
    }
}

fn tool_error(call: &ToolCall, message: String) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        output: json!({ "error": message }),
        is_error: true,
    }
}

async fn connect_server(
    config: &McpServerConfig,
    proxy: &ProxySettings,
    shutdown: &CancellationToken,
) -> Result<ClientService, String> {
    let timeout = Duration::from_secs(config.startup_timeout_sec);
    let resolved_url = config.url.clone();
    let mut resolved_headers = HashMap::new();
    for (name, value) in &config.http_headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid MCP HTTP Header name {name:?}"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| format!("invalid MCP HTTP Header value for {name:?}"))?;
        resolved_headers.insert(name, value);
    }
    for (name, variable) in &config.env_http_headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid MCP HTTP Header name {name:?}"))?;
        let resolved = resolve_env_variable(variable).map_err(|error| error.to_string())?;
        let value = reqwest::header::HeaderValue::from_str(&resolved)
            .map_err(|_| format!("invalid MCP HTTP Header value for {name:?}"))?;
        resolved_headers.insert(name, value);
    }
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(resolved_url)
        .custom_headers(resolved_headers)
        .reinit_on_expired_session(true);
    if let Some(variable) = &config.bearer_token_env_var {
        transport_config = transport_config
            .auth_header(resolve_env_variable(variable).map_err(|error| error.to_string())?);
    }
    let client = build_http_client(proxy)?;
    let transport = StreamableHttpClientTransport::with_client(client, transport_config);
    await_client_initialization(().serve(transport), timeout, shutdown).await
}

async fn await_client_initialization<F, E>(
    future: F,
    timeout: Duration,
    shutdown: &CancellationToken,
) -> Result<ClientService, String>
where
    F: std::future::Future<Output = Result<ClientService, E>>,
    E: fmt::Display,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err("MCP initialization was cancelled".to_owned()),
        result = tokio::time::timeout(timeout, &mut future) => match result {
            Ok(Ok(service)) => Ok(service),
            Ok(Err(error)) => Err(format!("MCP initialize failed: {error}")),
            Err(_) => Err(format!("MCP initialize exceeded {} seconds", timeout.as_secs())),
        }
    }
}

fn build_http_client(proxy: &ProxySettings) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy_url) = proxy
        .validated_url()
        .map_err(|error| format!("invalid global proxy configuration: {error}"))?
    {
        let configured = reqwest::Proxy::all(proxy_url.as_str())
            .map_err(|_| "failed to configure the global proxy for MCP HTTP".to_owned())?;
        builder = builder.proxy(configured);
    }
    builder
        .build()
        .map_err(|_| "failed to build the MCP HTTP client".to_owned())
}

async fn close_service(service: &mut ClientService) {
    match service.close_with_timeout(MCP_CLOSE_TIMEOUT).await {
        Ok(Some(_)) => {}
        Ok(None) => tracing::warn!("MCP connection cleanup exceeded close timeout"),
        Err(error) => tracing::warn!(error = %error, "MCP connection cleanup task failed"),
    }
}

fn redact_runtime_error(error: &str, config: &McpServerConfig, proxy: &ProxySettings) -> String {
    let mut redactor = SecretRedactor::new();
    redactor.add_url(&config.url, true);
    for value in config.http_headers.values() {
        redactor.add_configured(value);
    }
    for variable in config.env_http_headers.values() {
        if let Ok(value) = resolve_env_variable(variable) {
            redactor.add_configured(&value);
        }
    }
    if let Some(variable) = &config.bearer_token_env_var
        && let Ok(value) = resolve_env_variable(variable)
    {
        redactor.add_configured(&value);
    }
    if let Some(proxy_url) = proxy.url.as_ref() {
        redactor.add_url(proxy_url, false);
    }
    redactor.redact(error).chars().take(2_000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    fn http_config(name: &str, url: String) -> McpServerConfig {
        McpServerConfig {
            name: name.to_owned(),
            url,
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            enabled: true,
            startup_timeout_sec: 2,
        }
    }

    #[test]
    fn runtime_errors_redact_remote_and_proxy_credentials() {
        let config = McpServerConfig {
            url: "https://mcp.example/mcp?api_key=query-secret".to_owned(),
            http_headers: BTreeMap::from([
                ("x-api-key".to_owned(), "header-secret".to_owned()),
                ("authorization".to_owned(), "bearer-secret".to_owned()),
            ]),
            ..http_config("exa", "https://mcp.example/mcp".to_owned())
        };
        let proxy = ProxySettings {
            mode: crate::config::ProxyMode::Http,
            url: Some("http://user:proxy-secret@127.0.0.1:7890".to_owned()),
        };
        let error = redact_runtime_error(
            "query-secret header-secret bearer-secret proxy-secret",
            &config,
            &proxy,
        );
        for secret in [
            "query-secret",
            "header-secret",
            "bearer-secret",
            "proxy-secret",
        ] {
            assert!(!error.contains(secret));
        }
    }

    #[tokio::test]
    async fn remote_http_initializes_reports_capabilities_and_shuts_down() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_shutdown = CancellationToken::new();
        let server_task = tokio::spawn(mock_http_server(listener, server_shutdown.clone()));
        let (events, _receiver) = mpsc::unbounded_channel();
        let registry = McpRegistry::new(events);
        registry.reconfigure(
            &[http_config("fixture", format!("http://{address}/mcp"))],
            &ProxySettings::default(),
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if registry.snapshot("fixture").is_some_and(|snapshot| {
                    snapshot.status == McpServerStatus::Connected
                        && snapshot.capabilities.resources
                        && snapshot.capabilities.prompts
                        && snapshot.capabilities.tools
                        && snapshot.capabilities.tool_count == 2
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let tools = registry.tools();
        assert_eq!(tools.len(), 2);
        let weather = tools
            .iter()
            .find(|tool| tool.remote_name == "weather.search")
            .unwrap();
        assert!(weather.model_name.len() <= 64);
        assert!(
            weather.model_name.chars().all(
                |character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            )
        );
        let call = ToolCall {
            index: 0,
            id: "call-1".to_owned(),
            provider_id: Some("call-1".to_owned()),
            name: weather.model_name.clone(),
            arguments: json!({ "city": "Los Angeles" }),
        };
        let result = registry
            .call_tool(&call, &CancellationToken::new())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output["content"][0]["text"], "sunny");
        registry.shutdown().await;
        server_shutdown.cancel();
        server_task.await.unwrap();
    }

    async fn mock_http_server(listener: TcpListener, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    tokio::spawn(async move {
                        let _ = handle_http_connection(stream).await;
                    });
                }
            }
        }
    }

    async fn handle_http_connection(mut stream: TcpStream) -> std::io::Result<()> {
        let mut request = Vec::new();
        let mut chunk = [0u8; 4_096];
        let header_end = loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            request.extend_from_slice(&chunk[..read]);
            if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        if headers
            .lines()
            .next()
            .unwrap_or_default()
            .starts_with("DELETE ")
        {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await?;
            return Ok(());
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while request.len().saturating_sub(header_end) < content_length {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
        }
        let body = &request[header_end..request.len().min(header_end + content_length)];
        let message: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let Some(id) = message.get("id").cloned() else {
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        };
        let result = match message.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({
                "protocolVersion":message["params"]["protocolVersion"],
                "capabilities":{"resources":{},"prompts":{},"tools":{}},
                "serverInfo":{"name":"fixture-server","version":"1.0.0"}
            }),
            Some("tools/list") if message["params"]["cursor"] == "page-2" => json!({
                "tools":[{
                    "name":"clock.now",
                    "description":"Read the current time",
                    "inputSchema":{"type":"object","properties":{}}
                }]
            }),
            Some("tools/list") => json!({
                "tools":[{
                    "name":"weather.search",
                    "description":"Look up current weather",
                    "inputSchema":{
                        "type":"object",
                        "properties":{"city":{"type":"string"}},
                        "required":["city"]
                    }
                }],
                "nextCursor":"page-2"
            }),
            Some("tools/call") => {
                assert_eq!(message["params"]["name"], "weather.search");
                assert_eq!(message["params"]["arguments"]["city"], "Los Angeles");
                json!({"content":[{"type":"text","text":"sunny"}],"isError":false})
            }
            method => panic!("unexpected MCP request: {method:?}"),
        };
        let response = serde_json::to_vec(&json!({
            "jsonrpc":"2.0",
            "id":id,
            "result":result
        }))
        .unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&response).await
    }
}
