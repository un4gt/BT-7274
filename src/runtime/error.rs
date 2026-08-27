//! 运行时错误分类与可持久化的脱敏快照。

use color_eyre::Report;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorKind {
    Authentication,
    RateLimit,
    Timeout,
    Provider,
    Protocol,
    ContextOverflow,
    Mcp,
    Configuration,
    Cancelled,
    Unknown,
}

impl RuntimeErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::Provider => "provider",
            Self::Protocol => "protocol",
            Self::ContextOverflow => "context_overflow",
            Self::Mcp => "mcp",
            Self::Configuration => "configuration",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub summary: String,
    /// 已脱敏的诊断链；允许在详情弹窗展示，不直接写入聊天正文。
    pub detail: String,
    pub request_id: Option<String>,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeErrorSnapshot {
    pub kind: RuntimeErrorKind,
    pub summary: String,
}

impl RuntimeError {
    pub fn from_report(report: &Report, redacted_detail: String) -> Self {
        let mut kind = RuntimeErrorKind::Unknown;
        let mut request_id = None;
        let mut retry_after_ms = None;
        for cause in report.chain() {
            if let Some(error) = cause.downcast_ref::<HttpStatusError>() {
                kind = match error.status {
                    401 | 403 => RuntimeErrorKind::Authentication,
                    429 => RuntimeErrorKind::RateLimit,
                    _ => RuntimeErrorKind::Provider,
                };
                request_id = error.request_id.clone();
                retry_after_ms = error.retry_after_ms;
                break;
            }
            if cause.downcast_ref::<NetworkTimeoutError>().is_some() {
                kind = RuntimeErrorKind::Timeout;
                break;
            }
            if cause.downcast_ref::<CancellationError>().is_some() {
                kind = RuntimeErrorKind::Cancelled;
                break;
            }
            if cause.downcast_ref::<ContextOverflowError>().is_some() {
                kind = RuntimeErrorKind::ContextOverflow;
                break;
            }
        }
        if kind == RuntimeErrorKind::Unknown {
            let searchable = redacted_detail.to_ascii_lowercase();
            kind = if searchable.contains("timed out") || searchable.contains("timeout") {
                RuntimeErrorKind::Timeout
            } else if searchable.contains("context") && searchable.contains("exceed") {
                RuntimeErrorKind::ContextOverflow
            } else if [
                "sse",
                "stream json",
                "stream event",
                "stream payload",
                "unknown event",
                " is missing ",
                "finish_reason",
                "finishreason",
                "missing stop_reason",
                "ended before",
            ]
            .iter()
            .any(|needle| searchable.contains(needle))
            {
                RuntimeErrorKind::Protocol
            } else if searchable.contains("mcp") {
                RuntimeErrorKind::Mcp
            } else if [
                "configuration",
                "base url",
                "custom http header",
                "request json",
                "reasoning settings",
            ]
            .iter()
            .any(|needle| searchable.contains(needle))
            {
                RuntimeErrorKind::Configuration
            } else {
                RuntimeErrorKind::Provider
            };
        }
        Self {
            kind,
            // summary 也必须来自已脱敏文本，不能重新读取可能含凭据的原始 error。
            summary: safe_line(&redacted_detail, 240),
            detail: safe_multiline(&redacted_detail, 4_000),
            request_id,
            retry_after_ms,
        }
    }

    pub fn snapshot(&self) -> RuntimeErrorSnapshot {
        RuntimeErrorSnapshot {
            kind: self.kind,
            summary: self.summary.clone(),
        }
    }
}

fn safe_line(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn safe_multiline(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .take(max_chars)
        .collect()
}

#[derive(Debug, Clone)]
pub struct HttpStatusError {
    pub operation: String,
    pub status: u16,
    pub error_type: Option<String>,
    pub error_code: Option<String>,
    pub request_id: Option<String>,
    pub retry_after_ms: Option<u64>,
    pub quota_or_billing: bool,
}

impl fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} failed with HTTP {}",
            self.operation, self.status
        )?;
        if let Some(error_type) = &self.error_type {
            write!(formatter, " (type={error_type}")?;
            if let Some(error_code) = &self.error_code {
                write!(formatter, ", code={error_code}")?;
            }
            formatter.write_str(")")?;
        } else if let Some(error_code) = &self.error_code {
            write!(formatter, " (code={error_code})")?;
        }
        if self.quota_or_billing {
            formatter.write_str("; quota or billing errors are not retried")?;
        }
        if let Some(request_id) = &self.request_id {
            write!(formatter, "; request_id={request_id}")?;
        }
        Ok(())
    }
}

impl Error for HttpStatusError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStage {
    Connect,
    ResponseHeaders,
    ResponseBody,
    StreamIdle,
    Overall,
}

#[derive(Debug, Clone)]
pub struct NetworkTimeoutError {
    pub stage: NetworkStage,
    pub seconds: u64,
}

impl fmt::Display for NetworkTimeoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "network {:?} timed out after {} seconds",
            self.stage, self.seconds
        )
    }
}

impl Error for NetworkTimeoutError {}

#[derive(Debug, Clone)]
pub struct CancellationError;

impl fmt::Display for CancellationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("task cancelled")
    }
}

impl Error for CancellationError {}

#[derive(Debug, Clone)]
pub struct ContextOverflowError {
    pub message: String,
}

impl fmt::Display for ContextOverflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ContextOverflowError {}

#[cfg(test)]
mod tests {
    use super::*;
    use color_eyre::eyre::Report;

    #[test]
    fn http_statuses_map_to_stable_categories() {
        for (status, expected) in [
            (401, RuntimeErrorKind::Authentication),
            (429, RuntimeErrorKind::RateLimit),
            (503, RuntimeErrorKind::Provider),
        ] {
            let report = Report::new(HttpStatusError {
                operation: "request".to_owned(),
                status,
                error_type: None,
                error_code: None,
                request_id: Some("req-test".to_owned()),
                retry_after_ms: Some(1000),
                quota_or_billing: false,
            });
            let error = RuntimeError::from_report(&report, report.to_string());
            assert_eq!(error.kind, expected);
            assert_eq!(error.request_id.as_deref(), Some("req-test"));
        }
    }

    #[test]
    fn protocol_details_are_bounded_and_control_characters_removed() {
        let report = color_eyre::eyre::eyre!("SSE unknown event");
        let error = RuntimeError::from_report(&report, format!("SSE\0{}", "x".repeat(8_000)));
        assert_eq!(error.kind, RuntimeErrorKind::Protocol);
        assert!(!error.detail.contains('\0'));
        assert!(error.detail.chars().count() <= 4_000);
    }

    #[test]
    fn typed_and_semantic_errors_cover_runtime_categories() {
        let timeout = Report::new(NetworkTimeoutError {
            stage: NetworkStage::StreamIdle,
            seconds: 5,
        });
        assert_eq!(
            RuntimeError::from_report(&timeout, timeout.to_string()).kind,
            RuntimeErrorKind::Timeout
        );

        let overflow = Report::new(ContextOverflowError {
            message: "context exceeds model limit".to_owned(),
        });
        assert_eq!(
            RuntimeError::from_report(&overflow, overflow.to_string()).kind,
            RuntimeErrorKind::ContextOverflow
        );

        for (detail, expected) in [
            ("MCP server disconnected", RuntimeErrorKind::Mcp),
            (
                "invalid custom HTTP header",
                RuntimeErrorKind::Configuration,
            ),
        ] {
            let report = color_eyre::eyre::eyre!(detail);
            assert_eq!(
                RuntimeError::from_report(&report, detail.to_owned()).kind,
                expected
            );
        }
    }

    #[test]
    fn summary_uses_redacted_detail_instead_of_original_report() {
        let report = color_eyre::eyre::eyre!("secret-token");
        let error = RuntimeError::from_report(&report, "[REDACTED API KEY]".to_owned());
        assert!(!error.summary.contains("secret-token"));
        assert!(error.summary.contains("REDACTED"));
    }
}
