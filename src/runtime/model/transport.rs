//! Provider 共享的 HTTP、超时、有限重试与 SSE framing。

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Context, ContextCompat, Result, bail};
use futures::StreamExt;
use reqwest::{
    Client, Request, Response, StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER, USER_AGENT},
};
use serde_json::Value;
use std::time::Duration;
use url::Url;

use crate::{
    config::{ApiKind, ModelParameters, Provider, ProxySettings, is_sensitive_header},
    runtime::{
        error::{CancellationError, HttpStatusError, NetworkStage, NetworkTimeoutError},
        task::CancellationToken,
    },
    secret::resolve_env_value,
};

const MAX_ERROR_BODY: usize = 64 * 1024;
const MAX_JSON_BODY: usize = 8 * 1024 * 1024;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
pub(crate) struct RequestPolicy {
    pub connect_timeout: Duration,
    pub first_byte_timeout: Duration,
    pub idle_timeout: Duration,
    pub overall_timeout: Duration,
    pub max_attempts: u8,
}

impl RequestPolicy {
    pub fn from_parameters(parameters: &ModelParameters) -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            first_byte_timeout: Duration::from_secs(
                parameters.first_byte_timeout_seconds.unwrap_or(30),
            ),
            idle_timeout: Duration::from_secs(parameters.idle_timeout_seconds.unwrap_or(45)),
            overall_timeout: Duration::from_secs(parameters.timeout_seconds.unwrap_or(300)),
            max_attempts: parameters.retry_max_attempts.unwrap_or(3).clamp(1, 5),
        }
    }
}

pub(crate) struct HttpTransport {
    client: Client,
    base_url: Url,
    pub policy: RequestPolicy,
    cancellation: CancellationToken,
}

impl HttpTransport {
    pub fn new(
        provider: &Provider,
        proxy: &ProxySettings,
        parameters: &ModelParameters,
    ) -> Result<Self> {
        Self::with_cancellation(provider, proxy, parameters, CancellationToken::new())
    }

    pub fn with_cancellation(
        provider: &Provider,
        proxy: &ProxySettings,
        parameters: &ModelParameters,
        cancellation: CancellationToken,
    ) -> Result<Self> {
        let policy = RequestPolicy::from_parameters(parameters);
        let headers = provider_default_headers(provider)?;

        let mut builder = Client::builder()
            .no_proxy()
            .connect_timeout(policy.connect_timeout)
            .default_headers(headers);
        if let Some(proxy_url) = proxy
            .validated_url()
            .map_err(|err| color_eyre::Report::new(err).wrap_err("invalid proxy configuration"))?
        {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy_url.as_str())
                    .context("failed to configure HTTP client proxy")?,
            );
        }
        let client = builder.build().context("failed to build HTTP client")?;

        let resolved_base_url = resolve_env_value(provider.effective_base_url())
            .context("failed to resolve Provider Base URL")?;
        let mut base_url = Url::parse(&resolved_base_url).context("invalid Provider Base URL")?;
        if !matches!(base_url.scheme(), "http" | "https") || base_url.host().is_none() {
            bail!("Provider Base URL must use http or https and include a host");
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            bail!("Provider Base URL cannot contain a query string or fragment");
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }

        Ok(Self {
            client,
            base_url,
            policy,
            cancellation,
        })
    }

    pub fn endpoint(&self, relative: &str) -> Result<Url> {
        self.base_url
            .join(relative)
            .with_context(|| format!("invalid Provider endpoint {relative:?}"))
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// 只在收到响应正文前、且服务明确返回可重试状态时重试。
    /// 连接建立失败也可安全重试；首包超时不重试，避免请求已被服务执行后重复计费。
    pub async fn send(&self, request: Request, operation: &str) -> Result<Response> {
        for attempt in 1..=self.policy.max_attempts {
            if self.cancellation.is_cancelled() {
                return Err(color_eyre::Report::new(CancellationError));
            }
            let current = request
                .try_clone()
                .context("HTTP request body cannot be replayed")?;
            let execute =
                tokio::time::timeout(self.policy.first_byte_timeout, self.client.execute(current));
            let response = match tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => {
                    return Err(color_eyre::Report::new(CancellationError));
                }
                result = execute => result,
            } {
                Ok(Ok(response)) => response,
                Ok(Err(error)) if error.is_connect() && attempt < self.policy.max_attempts => {
                    retry_sleep(attempt, None, &self.cancellation).await?;
                    continue;
                }
                Ok(Err(error)) => {
                    if error.is_timeout() {
                        return Err(color_eyre::Report::new(NetworkTimeoutError {
                            stage: if error.is_connect() {
                                NetworkStage::Connect
                            } else {
                                NetworkStage::ResponseHeaders
                            },
                            seconds: if error.is_connect() {
                                self.policy.connect_timeout.as_secs()
                            } else {
                                self.policy.first_byte_timeout.as_secs()
                            },
                        })
                        .wrap_err(format!("{operation} request failed")));
                    }
                    return Err(color_eyre::Report::new(error)
                        .wrap_err(format!("{operation} request failed")));
                }
                Err(_) => {
                    return Err(color_eyre::Report::new(NetworkTimeoutError {
                        stage: NetworkStage::ResponseHeaders,
                        seconds: self.policy.first_byte_timeout.as_secs(),
                    })
                    .wrap_err(format!(
                        "{operation} timed out waiting for response headers"
                    )));
                }
            };

            if response.status().is_success() {
                if let Some(request_id) = response_request_id(response.headers()) {
                    tracing::debug!(operation, request_id = %request_id, "Provider 请求已接受");
                }
                return Ok(response);
            }

            let status = response.status();
            let retry_after = parse_retry_after(response.headers());
            let request_id = response_request_id(response.headers());
            let body = read_limited(
                response,
                MAX_ERROR_BODY,
                self.policy.idle_timeout,
                &self.cancellation,
            )
            .await?;
            let detail = classify_error(&body);
            let retryable = is_retryable_status(status) && !detail.quota_or_billing;
            if retryable && attempt < self.policy.max_attempts {
                tracing::warn!(
                    operation,
                    status = status.as_u16(),
                    attempt,
                    max_attempts = self.policy.max_attempts,
                    retry_after_ms = retry_after.map(|delay| delay.as_millis() as u64),
                    request_id = ?request_id,
                    error_type = ?detail.error_type,
                    error_code = ?detail.error_code,
                    "Provider 请求将在退避后重试"
                );
                retry_sleep(attempt, retry_after, &self.cancellation).await?;
                continue;
            }
            return Err(color_eyre::Report::new(HttpStatusError {
                operation: operation.to_owned(),
                status: status.as_u16(),
                error_type: detail.error_type,
                error_code: detail.error_code,
                request_id,
                retry_after_ms: retry_after.map(|delay| delay.as_millis() as u64),
                quota_or_billing: detail.quota_or_billing,
            }));
        }
        unreachable!("attempt range is never empty")
    }

    pub async fn json(&self, response: Response, operation: &str) -> Result<Value> {
        let body = read_limited(
            response,
            MAX_JSON_BODY,
            self.policy.idle_timeout,
            &self.cancellation,
        )
        .await?;
        serde_json::from_slice(&body).with_context(|| format!("invalid JSON from {operation}"))
    }
}

fn provider_default_headers(provider: &Provider) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&format!(
            "{}/{}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ))?,
    );
    for (name, value) in &provider.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid custom HTTP header name {name:?}"))?;
        let resolved = resolve_env_value(value)
            .with_context(|| format!("failed to resolve custom HTTP header {name:?}"))?;
        let mut value = HeaderValue::from_str(&resolved)
            .with_context(|| format!("invalid value for custom HTTP header {name:?}"))?;
        value.set_sensitive(is_sensitive_header(name.as_str()));
        headers.insert(name, value);
    }

    if let Some(api_key) = provider
        .resolved_api_key()
        .context("failed to resolve Provider API key")?
    {
        match provider.api_kind {
            ApiKind::ChatCompletions | ApiKind::Responses => {
                let mut value = HeaderValue::from_str(&format!("Bearer {api_key}"))?;
                value.set_sensitive(true);
                headers.insert(AUTHORIZATION, value);
            }
            ApiKind::AnthropicMessages => {
                let mut value = HeaderValue::from_str(&api_key)?;
                value.set_sensitive(true);
                headers.insert(HeaderName::from_static("x-api-key"), value);
            }
            ApiKind::GeminiGenerateContent => {
                let mut value = HeaderValue::from_str(&api_key)?;
                value.set_sensitive(true);
                headers.insert(HeaderName::from_static("x-goog-api-key"), value);
            }
        }
    }
    if provider.api_kind == ApiKind::AnthropicMessages && !headers.contains_key("anthropic-version")
    {
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        );
    }
    if provider.api_kind == ApiKind::GeminiGenerateContent {
        headers.insert(
            HeaderName::from_static("x-goog-api-client"),
            HeaderValue::from_static(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            )),
        );
    }
    Ok(headers)
}

pub(crate) fn response_request_id(headers: &HeaderMap) -> Option<String> {
    ["x-request-id", "request-id", "x-goog-request-id"]
        .into_iter()
        .find_map(|name| headers.get(name).and_then(|value| value.to_str().ok()))
        .map(|value| value.chars().take(128).collect())
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500 | 502 | 503 | 504)
}

async fn retry_sleep(
    attempt: u8,
    retry_after: Option<Duration>,
    cancellation: &CancellationToken,
) -> Result<()> {
    let exponential = Duration::from_millis(250 * (1_u64 << attempt.saturating_sub(1).min(5)));
    let delay = retry_after.unwrap_or(exponential).min(MAX_RETRY_AFTER);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(color_eyre::Report::new(CancellationError)),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER));
    }
    let at = DateTime::parse_from_rfc2822(raw).ok()?.with_timezone(&Utc);
    at.signed_duration_since(Utc::now())
        .to_std()
        .ok()
        .map(|duration| duration.min(MAX_RETRY_AFTER))
}

#[derive(Default)]
struct ErrorDetail {
    error_type: Option<String>,
    error_code: Option<String>,
    quota_or_billing: bool,
}

fn classify_error(body: &[u8]) -> ErrorDetail {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return ErrorDetail::default();
    };
    let error = value.get("error").unwrap_or(&value);
    let string_value = |name: &str| {
        error.get(name).and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
    };
    let error_type = string_value("type")
        .or_else(|| string_value("status"))
        .map(|value| value.chars().take(128).collect());
    let error_code = string_value("code").map(|value| value.chars().take(128).collect());
    let searchable = [
        error_type.as_deref().unwrap_or_default(),
        error_code.as_deref().unwrap_or_default(),
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ]
    .join(" ")
    .to_ascii_lowercase();
    let quota_or_billing = ["quota", "billing", "insufficient_quota", "credit balance"]
        .iter()
        .any(|needle| searchable.contains(needle));
    ErrorDetail {
        error_type,
        error_code,
        quota_or_billing,
    }
}

async fn read_limited(
    response: Response,
    limit: usize,
    idle_timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut output = Vec::new();
    loop {
        let wait = tokio::time::timeout(idle_timeout, stream.next());
        let next = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(color_eyre::Report::new(CancellationError));
            }
            result = wait => result.map_err(|_| {
                color_eyre::Report::new(NetworkTimeoutError {
                    stage: NetworkStage::ResponseBody,
                    seconds: idle_timeout.as_secs(),
                })
            })?,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.context("failed to read response body")?;
        if output.len().saturating_add(chunk.len()) > limit {
            bail!("response body exceeds {limit} bytes");
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub(crate) struct SseDecoder {
    pending: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.pending.drain(..=index).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events)?;
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<SseEvent>> {
        let mut events = Vec::new();
        if !self.pending.is_empty() {
            let mut line = std::mem::take(&mut self.pending);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events)?;
        }
        self.dispatch(&mut events);
        Ok(events)
    }

    fn process_line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) -> Result<()> {
        if line.is_empty() {
            self.dispatch(events);
            return Ok(());
        }
        if line[0] == b':' {
            return Ok(());
        }
        let line = std::str::from_utf8(line).context("SSE stream contains invalid UTF-8")?;
        let (field, raw_value) = line.split_once(':').unwrap_or((line, ""));
        let value = raw_value.strip_prefix(' ').unwrap_or(raw_value);
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            // id/retry 和未来字段不改变业务 payload。
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        if self.event.is_some() || !self.data.is_empty() {
            events.push(SseEvent {
                event: self.event.take().filter(|event| !event.is_empty()),
                data: std::mem::take(&mut self.data).join("\n"),
            });
        } else {
            self.event = None;
        }
    }
}

pub(crate) async fn consume_sse<F>(
    response: Response,
    idle_timeout: Duration,
    cancellation: &CancellationToken,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(SseEvent) -> Result<()>,
{
    let mut decoder = SseDecoder::default();
    let mut stream = response.bytes_stream();
    loop {
        let wait = tokio::time::timeout(idle_timeout, stream.next());
        let next = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(color_eyre::Report::new(CancellationError));
            }
            result = wait => result.map_err(|_| {
                color_eyre::Report::new(NetworkTimeoutError {
                    stage: NetworkStage::StreamIdle,
                    seconds: idle_timeout.as_secs(),
                })
            })?,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.context("failed to read SSE response")?;
        for event in decoder.feed(&chunk)? {
            on_event(event)?;
        }
    }
    for event in decoder.finish()? {
        on_event(event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_headers_include_api_client_identity() {
        let mut provider = Provider::new(ApiKind::GeminiGenerateContent);
        provider.api_key = Some(crate::secret::SecretValue::from("test-key"));

        let headers = provider_default_headers(&provider).unwrap();

        assert_eq!(
            headers
                .get("x-goog-api-client")
                .and_then(|value| value.to_str().ok()),
            Some(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
        );
        assert_eq!(
            headers
                .get("x-goog-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("test-key")
        );
    }

    #[test]
    fn decoder_handles_crlf_chunks_comments_and_multiline_data() {
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for chunk in [
            b": ping\r\nevent: sample\r\ndata: first".as_slice(),
            b"\r\ndata: second\r\n\r\n".as_slice(),
        ] {
            events.extend(decoder.feed(chunk).unwrap());
        }
        events.extend(decoder.finish().unwrap());
        assert_eq!(
            events,
            [SseEvent {
                event: Some("sample".to_owned()),
                data: "first\nsecond".to_owned(),
            }]
        );
    }

    #[test]
    fn decoder_preserves_utf8_split_across_chunks() {
        let raw = "data: 你好\n\n".as_bytes();
        let split = raw.iter().position(|byte| *byte >= 0x80).unwrap() + 1;
        let mut decoder = SseDecoder::default();
        assert!(decoder.feed(&raw[..split]).unwrap().is_empty());
        let events = decoder.feed(&raw[split..]).unwrap();
        assert_eq!(events[0].data, "你好");
    }

    #[test]
    fn quota_errors_are_classified_without_exposing_message_text() {
        let detail = classify_error(
            br#"{"error":{"type":"insufficient_quota","code":"billing_hard_limit","message":"private detail"}}"#,
        );
        assert!(detail.quota_or_billing);
        assert_eq!(detail.error_type.as_deref(), Some("insufficient_quota"));
        assert_eq!(detail.error_code.as_deref(), Some("billing_hard_limit"));
    }

    #[test]
    fn retry_after_seconds_are_bounded() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(parse_retry_after(&headers), Some(MAX_RETRY_AFTER));
    }

    #[test]
    fn request_id_uses_known_provider_headers_only() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-test"));
        assert_eq!(response_request_id(&headers).as_deref(), Some("req-test"));
    }

    #[tokio::test]
    async fn retry_backoff_is_cancelled_without_waiting_for_the_delay() {
        let token = CancellationToken::new();
        token.cancel();
        let result = retry_sleep(5, Some(MAX_RETRY_AFTER), &token).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .chain()
                .any(|cause| cause.downcast_ref::<CancellationError>().is_some())
        );
    }
}
