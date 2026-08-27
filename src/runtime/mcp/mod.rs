//! 远程 MCP 连接管理。
//!
//! 这里只支持 Streamable HTTP；服务端可用 JSON 或 SSE 返回流式响应。
//! Runtime 只负责初始化、状态监测、重连和关闭，不调用 MCP 远程能力。

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{
    RoleClient, ServiceExt,
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    config::{McpCapabilityMetadata, McpServerConfig, McpTransportConfig, ProxySettings},
    event::{AppEvent, Event},
    runtime::task::CancellationToken,
    secret::{SecretRedactor, resolve_env_value},
};

const MCP_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);

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
    pub id: String,
    pub name: String,
    pub status: McpServerStatus,
    pub generation: u64,
    pub capabilities: McpCapabilityMetadata,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRuntimeEvent {
    Changed { server_id: String, generation: u64 },
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
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
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
                state.servers.insert(
                    config.id.clone(),
                    ServerEntry {
                        snapshot: McpServerSnapshot {
                            id: config.id.clone(),
                            name: config.name.clone(),
                            status: if config.enabled {
                                McpServerStatus::Starting
                            } else {
                                McpServerStatus::Disabled
                            },
                            generation,
                            capabilities: config.capabilities.clone(),
                            error: None,
                        },
                        shutdown: shutdown.clone(),
                        task: None,
                    },
                );
                changed.push((config.id.clone(), generation));
                if config.enabled {
                    launches.push((config, proxy.clone(), generation, shutdown));
                }
            }
        }
        for (config, proxy, generation, shutdown) in launches {
            self.spawn_actor(config, proxy, generation, shutdown);
        }
        for (server_id, generation) in changed {
            self.emit_changed(server_id, generation);
        }
    }

    pub fn reconnect(&self, config: McpServerConfig, proxy: ProxySettings) -> Result<(), String> {
        if !config.enabled {
            return Err(format!("MCP Server {:?} is disabled", config.id));
        }
        config.validate().map_err(|error| format!("{error:#}"))?;
        let (generation, shutdown) = {
            let mut state = self.lock_state();
            if state.shutting_down {
                return Err("MCP Registry is shutting down".to_owned());
            }
            if let Some(mut old) = state.servers.remove(&config.id) {
                old.shutdown.cancel();
                if let Some(task) = old.task.take() {
                    state.retired.push(task);
                }
            }
            state.next_generation = state.next_generation.wrapping_add(1);
            let generation = state.next_generation;
            let shutdown = CancellationToken::new();
            state.servers.insert(
                config.id.clone(),
                ServerEntry {
                    snapshot: McpServerSnapshot {
                        id: config.id.clone(),
                        name: config.name.clone(),
                        status: McpServerStatus::Starting,
                        generation,
                        capabilities: config.capabilities.clone(),
                        error: None,
                    },
                    shutdown: shutdown.clone(),
                    task: None,
                },
            );
            (generation, shutdown)
        };
        self.spawn_actor(config.clone(), proxy, generation, shutdown);
        self.emit_changed(config.id, generation);
        Ok(())
    }

    pub fn snapshots(&self) -> BTreeMap<String, McpServerSnapshot> {
        self.lock_state()
            .servers
            .iter()
            .map(|(id, entry)| (id.clone(), entry.snapshot.clone()))
            .collect()
    }

    pub fn snapshot(&self, server_id: &str) -> Option<McpServerSnapshot> {
        self.lock_state()
            .servers
            .get(server_id)
            .map(|entry| entry.snapshot.clone())
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
    ) {
        let registry = self.clone();
        let server_id = config.id.clone();
        let task = tokio::spawn(async move {
            server_actor(registry, config, proxy, generation, shutdown).await;
        });
        let mut state = self.lock_state();
        if let Some(entry) = state
            .servers
            .get_mut(&server_id)
            .filter(|entry| entry.snapshot.generation == generation)
        {
            entry.task = Some(task);
        } else {
            task.abort();
        }
    }

    fn set_connected(&self, server_id: &str, generation: u64, capabilities: McpCapabilityMetadata) {
        let mut state = self.lock_state();
        let Some(entry) = state
            .servers
            .get_mut(server_id)
            .filter(|entry| entry.snapshot.generation == generation)
        else {
            return;
        };
        entry.snapshot.status = McpServerStatus::Connected;
        entry.snapshot.capabilities = capabilities;
        entry.snapshot.error = None;
        drop(state);
        self.emit_changed(server_id.to_owned(), generation);
    }

    fn set_failed(&self, server_id: &str, generation: u64, error: String) {
        let mut state = self.lock_state();
        let Some(entry) = state
            .servers
            .get_mut(server_id)
            .filter(|entry| entry.snapshot.generation == generation)
        else {
            return;
        };
        entry.snapshot.status = McpServerStatus::Failed;
        entry.snapshot.error = Some(error);
        drop(state);
        self.emit_changed(server_id.to_owned(), generation);
    }

    fn emit_changed(&self, server_id: String, generation: u64) {
        let _ =
            self.inner
                .events
                .send(Event::App(AppEvent::McpRuntime(McpRuntimeEvent::Changed {
                    server_id,
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
) {
    tracing::info!(server = %config.id, transport = "streamable_http", "MCP Server 开始连接");
    let mut service = match connect_server(&config, &proxy, &shutdown).await {
        Ok(service) => service,
        Err(error) => {
            let error = redact_runtime_error(&error, &config, &proxy);
            tracing::error!(server = %config.id, error = %error, "MCP Server 连接失败");
            registry.set_failed(&config.id, generation, error);
            return;
        }
    };

    let Some(peer) = service.peer_info() else {
        let error = "MCP initialize response did not include peer metadata".to_owned();
        registry.set_failed(&config.id, generation, error);
        close_service(&mut service).await;
        return;
    };
    let capabilities = McpCapabilityMetadata {
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
    };
    registry.set_connected(&config.id, generation, capabilities);
    tracing::info!(server = %config.id, "MCP Server 已连接");

    let mut health = tokio::time::interval(Duration::from_millis(250));
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = health.tick() => {
                if service.is_closed() {
                    registry.set_failed(
                        &config.id,
                        generation,
                        format!("MCP Server {:?} transport closed unexpectedly", config.id),
                    );
                    break;
                }
            }
        }
    }
    close_service(&mut service).await;
    tracing::info!(server = %config.id, "MCP Server 连接已关闭");
}

async fn connect_server(
    config: &McpServerConfig,
    proxy: &ProxySettings,
    shutdown: &CancellationToken,
) -> Result<ClientService, String> {
    let timeout = Duration::from_secs(config.timeout_seconds);
    let McpTransportConfig::StreamableHttp {
        url,
        headers,
        auth_token,
    } = &config.transport;
    let resolved_url = resolve_env_value(url).map_err(|error| error.to_string())?;
    let mut resolved_headers = HashMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid MCP HTTP Header name {name:?}"))?;
        let resolved = resolve_env_value(value).map_err(|error| error.to_string())?;
        let value = reqwest::header::HeaderValue::from_str(&resolved)
            .map_err(|_| format!("invalid MCP HTTP Header value for {name:?}"))?;
        resolved_headers.insert(name, value);
    }
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(resolved_url)
        .custom_headers(resolved_headers)
        .reinit_on_expired_session(true);
    if let Some(token) = auth_token {
        transport_config = transport_config
            .auth_header(resolve_env_value(token).map_err(|error| error.to_string())?);
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
    let McpTransportConfig::StreamableHttp {
        url,
        headers,
        auth_token,
    } = &config.transport;
    redactor.add_url(url, true);
    for value in headers.values() {
        redactor.add_configured(value);
    }
    if let Some(token) = auth_token {
        redactor.add_configured(token);
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

    fn http_config(id: &str, url: String) -> McpServerConfig {
        McpServerConfig {
            id: id.to_owned(),
            name: id.to_owned(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url,
                headers: BTreeMap::new(),
                auth_token: None,
            },
            timeout_seconds: 2,
            capabilities: Default::default(),
        }
    }

    #[test]
    fn runtime_errors_redact_remote_and_proxy_credentials() {
        let config = McpServerConfig {
            transport: McpTransportConfig::StreamableHttp {
                url: "https://mcp.example/mcp?api_key=query-secret".to_owned(),
                headers: BTreeMap::from([("x-api-key".to_owned(), "header-secret".to_owned())]),
                auth_token: Some("bearer-secret".to_owned()),
            },
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
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
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
        let response = serde_json::to_vec(&json!({
            "jsonrpc":"2.0",
            "id":id,
            "result":{
                "protocolVersion":message["params"]["protocolVersion"],
                "capabilities":{"resources":{},"prompts":{}},
                "serverInfo":{"name":"fixture-server","version":"1.0.0"}
            }
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
