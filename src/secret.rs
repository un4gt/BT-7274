//! Secret sources, environment resolution, and diagnostic redaction.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, ops::Deref};
use url::Url;

pub const REDACTED: &str = "[REDACTED]";

/// A persistable secret source. The stored value may be a literal or a full
/// environment reference such as `${OPENAI_API_KEY}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn resolve(&self) -> Result<String, SecretResolveError> {
        resolve_env_value(&self.0)
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

impl Deref for SecretValue {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.expose()
    }
}

impl AsRef<str> for SecretValue {
    fn as_ref(&self) -> &str {
        self.expose()
    }
}

impl From<String> for SecretValue {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretValue {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretResolveError {
    environment_variable: String,
}

impl fmt::Display for SecretResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "required environment variable {} is unavailable or is not valid Unicode",
            self.environment_variable
        )
    }
}

impl std::error::Error for SecretResolveError {}

pub fn env_reference_name(value: &str) -> Option<&str> {
    let name = value.strip_prefix("${")?.strip_suffix('}')?;
    valid_env_name(name).then_some(name)
}

pub fn resolve_env_value(value: &str) -> Result<String, SecretResolveError> {
    let Some(name) = env_reference_name(value) else {
        return Ok(value.to_owned());
    };
    resolve_env_variable(name)
}

pub(crate) fn resolve_env_variable(name: &str) -> Result<String, SecretResolveError> {
    std::env::var(name).map_err(|_| SecretResolveError {
        environment_variable: name.to_owned(),
    })
}

pub(crate) fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('A'..='Z' | 'a'..='z' | '_'))
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

/// A deterministic redactor built from all secrets known at a trust boundary.
#[derive(Clone, Default)]
pub struct SecretRedactor {
    replacements: Vec<(String, String)>,
}

impl fmt::Debug for SecretRedactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretRedactor")
            .field("replacement_count", &self.replacements.len())
            .finish()
    }
}

impl SecretRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a configured secret and its resolved value without mutating the
    /// configured source. Missing environment variables are intentionally
    /// ignored here so redaction never turns a diagnostic failure into another
    /// failure.
    pub fn add_configured(&mut self, value: &str) {
        if env_reference_name(value).is_some() {
            if let Ok(resolved) = resolve_env_value(value) {
                self.add_secret(&resolved);
            }
        } else {
            self.add_secret(value);
        }
    }

    pub fn add_secret(&mut self, value: &str) {
        self.add_replacement(value, REDACTED);
    }

    pub fn add_header(&mut self, name: &str, value: &str) {
        if is_sensitive_header(name) {
            self.add_configured(value);
        }
    }

    /// Register URL userinfo and query values. `all_query_values` is used for
    /// MCP endpoints, whose query string is treated as authentication material.
    pub fn add_url(&mut self, raw: &str, all_query_values: bool) {
        let resolved = resolve_env_value(raw).unwrap_or_else(|_| raw.to_owned());
        let Ok(mut sanitized) = Url::parse(&resolved) else {
            return;
        };
        let mut changed = false;
        let username = sanitized.username().to_owned();
        let password = sanitized.password().map(str::to_owned);
        if !username.is_empty() {
            self.add_secret(&username);
            let _ = sanitized.set_username(REDACTED);
            changed = true;
        }
        if let Some(password) = password {
            self.add_secret(&password);
            let _ = sanitized.set_password(Some(REDACTED));
            changed = true;
        }

        let pairs = sanitized
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        if !pairs.is_empty() {
            let mut sanitized_pairs = Vec::with_capacity(pairs.len());
            for (name, value) in pairs {
                if all_query_values || sensitive_key(&name) {
                    self.add_secret(&value);
                    sanitized_pairs.push((name, REDACTED.to_owned()));
                    changed = true;
                } else {
                    sanitized_pairs.push((name, value));
                }
            }
            if changed {
                sanitized
                    .query_pairs_mut()
                    .clear()
                    .extend_pairs(sanitized_pairs);
            }
        }
        if changed {
            self.add_replacement(&resolved, sanitized.as_str());
            if resolved != raw {
                self.add_replacement(raw, sanitized.as_str());
            }
        }
    }

    pub fn redact(&self, value: &str) -> String {
        let mut replacements = self.replacements.clone();
        replacements.sort_by_key(|(source, _)| std::cmp::Reverse(source.len()));
        replacements.dedup_by(|left, right| left.0 == right.0);
        replacements
            .into_iter()
            .fold(value.to_owned(), |text, (source, replacement)| {
                text.replace(&source, &replacement)
            })
    }

    fn add_replacement(&mut self, source: &str, replacement: &str) {
        if source.chars().count() < 3 || source == REDACTED {
            return;
        }
        self.replacements
            .push((source.to_owned(), replacement.to_owned()));
    }
}

pub fn is_sensitive_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-goog-api-key"
    ) || name.ends_with("-api-key")
        || name.ends_with("-token")
        || name.ends_with("-secret")
}

pub fn sensitive_key(key: &str) -> bool {
    let normalized = normalized_key(key);
    [
        "apikey",
        "authorization",
        "password",
        "passwd",
        "secret",
        "token",
        "cookie",
        "credential",
        "privatekey",
        "httpproxy",
        "httpsproxy",
        "allproxy",
        "proxyauth",
        "sshauthsock",
    ]
    .iter()
    .any(|candidate| normalized.contains(candidate))
}

fn sensitive_container(key: &str) -> bool {
    matches!(
        normalized_key(key).as_str(),
        "env" | "environment" | "headers"
    )
}

fn normalized_key(key: &str) -> String {
    key.to_ascii_lowercase().replace(['-', '_'], "")
}

pub fn redact_json(value: &Value) -> Value {
    redact_json_inner(value, false)
}

fn redact_json_inner(value: &Value, inherited_sensitive: bool) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let sensitive =
                        inherited_sensitive || sensitive_key(key) || sensitive_container(key);
                    (key.clone(), redact_json_inner(value, sensitive))
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| redact_json_inner(value, inherited_sensitive))
                .collect(),
        ),
        Value::String(_) if inherited_sensitive => Value::String(REDACTED.to_owned()),
        Value::String(value) => Value::String(redact_url_value(&bounded_chars(value, 8_000))),
        other => other.clone(),
    }
}

pub fn redact_url(raw: &str, all_query_values: bool) -> String {
    if env_reference_name(raw).is_some() {
        return raw.to_owned();
    }
    let mut redactor = SecretRedactor::new();
    redactor.add_url(raw, all_query_values);
    redactor.redact(raw)
}

fn redact_url_value(value: &str) -> String {
    let mut redactor = SecretRedactor::new();
    redactor.add_url(value, false);
    redactor.redact(value)
}

fn bounded_chars(value: &str, limit: usize) -> String {
    let mut bounded = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_debug_display_and_serialization_never_resolve_the_source() {
        let secret = SecretValue::from("${PATH}");
        assert_eq!(format!("{secret:?}"), "SecretValue([REDACTED])");
        assert_eq!(secret.to_string(), REDACTED);
        assert_eq!(serde_json::to_string(&secret).unwrap(), r#""${PATH}""#);
        assert_eq!(secret.resolve().unwrap(), std::env::var("PATH").unwrap());
        assert_eq!(secret.expose(), "${PATH}");
    }

    #[test]
    fn missing_environment_errors_name_only_the_reference() {
        let name = "BT_7274_TEST_ENV_THAT_MUST_NOT_EXIST_7F5C";
        let source = format!("${{{name}}}");
        let error = resolve_env_value(&source).unwrap_err();
        assert_eq!(error.environment_variable, name);
        assert!(!error.to_string().contains("secret-value"));
    }

    #[test]
    fn json_redaction_covers_headers_urls_env_and_nested_secrets() {
        let arguments = json!({
            "headers": {"Authorization": "Bearer header-secret"},
            "env": {"NORMAL_NAME": "environment-secret"},
            "nested": {"password": "account-password"},
            "url": "https://url-user:url-password@example.test/path?api_key=query-secret"
        });
        let json = redact_json(&arguments);
        assert_eq!(json["headers"]["Authorization"], REDACTED);
        assert_eq!(json["env"]["NORMAL_NAME"], REDACTED);
        assert_eq!(json["nested"]["password"], REDACTED);
        assert!(!json["url"].as_str().unwrap().contains("url-password"));
    }
}
