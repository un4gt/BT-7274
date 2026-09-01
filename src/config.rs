//! 应用配置：多供应商（Provider）模型管理，持久化为 TOML。
//!
//! 配置文件位于 `dirs::config_dir()/bt-7274/config.toml`。
//! 每个供应商自带协议类型 / Base URL / API Key / 已选模型列表；
//! 不兼容版本不会迁移；原文件备份后以当前默认配置启动。

use color_eyre::eyre::{Context, ContextCompat, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor},
    ser::SerializeMap,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};
use url::Url;
use uuid::Uuid;

pub use crate::secret::{env_reference_name, is_sensitive_header};
use crate::{
    i18n::Lang,
    secret::{SecretValue, redact_url, resolve_env_value, valid_env_name},
    storage::atomic_write_private,
};

/// 当前写出的配置结构版本。缺少该字段的历史配置视为 v0。
pub const CURRENT_CONFIG_VERSION: u32 = 8;

/// Provider 使用的原生 API 协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKind {
    /// `v1/chat/completions`
    #[default]
    ChatCompletions,
    /// `v1/responses`
    Responses,
    /// Anthropic `v1/messages`。
    AnthropicMessages,
    /// Gemini `v1beta/models/{model}:generateContent`。
    GeminiGenerateContent,
}

impl ApiKind {
    pub const ALL: [Self; 4] = [
        Self::ChatCompletions,
        Self::Responses,
        Self::AnthropicMessages,
        Self::GeminiGenerateContent,
    ];

    /// 设置界面与 footer 中展示的接口路径。
    pub fn label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "v1/chat/completions",
            Self::Responses => "v1/responses",
            Self::AnthropicMessages => "v1/messages",
            Self::GeminiGenerateContent => "v1beta/generateContent",
        }
    }

    /// footer 中的短标记。
    pub fn short_label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat",
            Self::Responses => "resp",
            Self::AnthropicMessages => "anth",
            Self::GeminiGenerateContent => "gem",
        }
    }

    pub fn default_name(self) -> &'static str {
        match self {
            Self::ChatCompletions | Self::Responses => "OpenAI",
            Self::AnthropicMessages => "Anthropic",
            Self::GeminiGenerateContent => "Gemini",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::ChatCompletions | Self::Responses => "https://api.openai.com/v1",
            Self::AnthropicMessages => "https://api.anthropic.com/v1",
            Self::GeminiGenerateContent => "https://generativelanguage.googleapis.com/v1beta",
        }
    }

    pub fn api_key_envs(self) -> &'static [&'static str] {
        match self {
            Self::ChatCompletions | Self::Responses => &["OPENAI_API_KEY"],
            Self::AnthropicMessages => &["ANTHROPIC_API_KEY"],
            Self::GeminiGenerateContent => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }
}

/// 模型对某能力的支持状态。`Unknown` 保持向后兼容：文本请求可以尝试，
/// 只有明确的 `Unsupported` 才会在本地阻止请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    #[default]
    Unknown,
    Supported,
    Unsupported,
}

/// 从 Provider 模型目录或用户配置获得的模型能力。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    #[serde(default = "supported_capability")]
    pub text: CapabilitySupport,
    #[serde(default)]
    pub vision: CapabilitySupport,
    #[serde(default)]
    pub audio: CapabilitySupport,
    #[serde(default)]
    pub reasoning: CapabilitySupport,
    #[serde(default)]
    pub structured_output: CapabilitySupport,
    #[serde(default = "supported_capability")]
    pub streaming: CapabilitySupport,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

const fn supported_capability() -> CapabilitySupport {
    CapabilitySupport::Supported
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            text: CapabilitySupport::Supported,
            vision: CapabilitySupport::Unknown,
            audio: CapabilitySupport::Unknown,
            reasoning: CapabilitySupport::Unknown,
            structured_output: CapabilitySupport::Unknown,
            streaming: CapabilitySupport::Supported,
            context_window: None,
            max_output_tokens: None,
        }
    }
}

/// Provider 原生 reasoning 参数。不同协议保持不同表示，不做伪等价转换。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReasoningSettings {
    OpenAi { effort: String },
    Anthropic { budget_tokens: u32 },
    Gemini { thinking_budget: i32 },
}

/// 每个模型独立的请求参数与网络可靠性覆盖项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelParameters {
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub reasoning: Option<ReasoningSettings>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub first_byte_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub idle_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub retry_max_attempts: Option<u8>,
    /// Provider 专用顶层 JSON 参数。结构字段和鉴权字段禁止覆盖。
    #[serde(default)]
    pub extra_body: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelSettings {
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub parameters: ModelParameters,
}

/// 可持久化的模型选择。`provider_name` 是历史快照；解析时以稳定 id 为准。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelection {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
}

/// 长会话上下文预算与自动压缩策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSettings {
    /// 达到阈值时是否在发送前自动压缩；压缩结果始终会在 UI 中提示。
    #[serde(default = "default_true")]
    pub auto_compact: bool,
    /// 已规划 token（输入 + 预留输出）占模型窗口的百分比阈值。
    #[serde(default = "default_auto_compact_threshold")]
    pub auto_compact_threshold_percent: u8,
    /// 模型没有显式 `max_output_tokens` 时预留的输出空间。
    #[serde(default = "default_reserved_output_tokens")]
    pub reserved_output_tokens: u32,
}

const fn default_true() -> bool {
    true
}

const fn default_auto_compact_threshold() -> u8 {
    85
}

const fn default_reserved_output_tokens() -> u32 {
    4096
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            auto_compact: default_true(),
            auto_compact_threshold_percent: default_auto_compact_threshold(),
            reserved_output_tokens: default_reserved_output_tokens(),
        }
    }
}

impl ContextSettings {
    pub fn normalize(&mut self) {
        self.auto_compact_threshold_percent = self.auto_compact_threshold_percent.clamp(50, 95);
        self.reserved_output_tokens = self.reserved_output_tokens.clamp(256, 262_144);
    }
}

/// 应用的视觉主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// 青绿主色与琥珀强调的默认深色主题。
    #[default]
    Vanguard,
    /// 冷灰底、蓝色主色的深色主题。
    Carbon,
    /// 高对比浅色主题。
    Paper,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::Vanguard, Self::Carbon, Self::Paper];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vanguard => "Vanguard",
            Self::Carbon => "Carbon",
            Self::Paper => "Paper",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BindingCode {
    Char(char),
    Enter,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Esc,
    Function(u8),
}

/// 可序列化的终端按键组合，例如 `enter`、`ctrl+left` 或 `alt+c`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    code: BindingCode,
    modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn matches(self, event: KeyEvent) -> bool {
        let relevant = event.modifiers
            & KeyModifiers::CONTROL
                .union(KeyModifiers::ALT)
                .union(KeyModifiers::SHIFT);
        relevant == self.modifiers && self.code_matches(event.code)
    }

    /// 方向类命令允许额外按住 Shift 来扩展选择范围。
    pub fn matches_with_optional_shift(self, event: KeyEvent) -> bool {
        let relevant = event.modifiers
            & KeyModifiers::CONTROL
                .union(KeyModifiers::ALT)
                .union(KeyModifiers::SHIFT);
        let modifiers_match = relevant == self.modifiers
            || (!self.modifiers.contains(KeyModifiers::SHIFT)
                && relevant == self.modifiers.union(KeyModifiers::SHIFT));
        modifiers_match && self.code_matches(event.code)
    }

    fn code_matches(self, code: KeyCode) -> bool {
        match (self.code, code) {
            (BindingCode::Char(expected), KeyCode::Char(actual)) => {
                expected.eq_ignore_ascii_case(&actual)
            }
            (BindingCode::Enter, KeyCode::Enter)
            | (BindingCode::Up, KeyCode::Up)
            | (BindingCode::Down, KeyCode::Down)
            | (BindingCode::Left, KeyCode::Left)
            | (BindingCode::Right, KeyCode::Right)
            | (BindingCode::Home, KeyCode::Home)
            | (BindingCode::End, KeyCode::End)
            | (BindingCode::PageUp, KeyCode::PageUp)
            | (BindingCode::PageDown, KeyCode::PageDown)
            | (BindingCode::Tab, KeyCode::Tab)
            | (BindingCode::BackTab, KeyCode::BackTab)
            | (BindingCode::Backspace, KeyCode::Backspace)
            | (BindingCode::Delete, KeyCode::Delete)
            | (BindingCode::Esc, KeyCode::Esc) => true,
            (BindingCode::Function(expected), KeyCode::F(actual)) => expected == actual,
            _ => false,
        }
    }

    fn intercepts_text_input(self) -> bool {
        matches!(self.code, BindingCode::Char(_))
            && !self
                .modifiers
                .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT))
    }

    fn is_reserved(self) -> bool {
        (self.modifiers == KeyModifiers::CONTROL
            && matches!(
                self.code,
                BindingCode::Char('c' | 'C' | 'f' | 'F' | 'm' | 'M')
            ))
            || (self.modifiers == KeyModifiers::ALT
                && matches!(self.code, BindingCode::Char('m' | 'M')))
            || (self.modifiers == KeyModifiers::NONE
                && matches!(self.code, BindingCode::Esc | BindingCode::Function(2..=4)))
    }
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            formatter.write_str("ctrl+")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            formatter.write_str("alt+")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            formatter.write_str("shift+")?;
        }
        match self.code {
            BindingCode::Char(' ') => formatter.write_str("space"),
            BindingCode::Char(character) => write!(formatter, "{character}"),
            BindingCode::Enter => formatter.write_str("enter"),
            BindingCode::Up => formatter.write_str("up"),
            BindingCode::Down => formatter.write_str("down"),
            BindingCode::Left => formatter.write_str("left"),
            BindingCode::Right => formatter.write_str("right"),
            BindingCode::Home => formatter.write_str("home"),
            BindingCode::End => formatter.write_str("end"),
            BindingCode::PageUp => formatter.write_str("pageup"),
            BindingCode::PageDown => formatter.write_str("pagedown"),
            BindingCode::Tab => formatter.write_str("tab"),
            BindingCode::BackTab => formatter.write_str("backtab"),
            BindingCode::Backspace => formatter.write_str("backspace"),
            BindingCode::Delete => formatter.write_str("delete"),
            BindingCode::Esc => formatter.write_str("esc"),
            BindingCode::Function(number) => write!(formatter, "f{number}"),
        }
    }
}

impl FromStr for KeyBinding {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let raw = raw.trim().to_ascii_lowercase();
        if raw.is_empty() {
            return Err("快捷键不能为空".to_owned());
        }
        let parts = raw.split('+').map(str::trim).collect::<Vec<_>>();
        let Some(key) = parts.last().copied().filter(|part| !part.is_empty()) else {
            return Err(format!("快捷键 {raw:?} 缺少按键"));
        };
        let mut modifiers = KeyModifiers::NONE;
        for modifier in &parts[..parts.len().saturating_sub(1)] {
            let value = match *modifier {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("快捷键 {raw:?} 包含未知修饰键 {modifier:?}")),
            };
            if modifiers.contains(value) {
                return Err(format!("快捷键 {raw:?} 重复声明修饰键 {modifier:?}"));
            }
            modifiers.insert(value);
        }
        let code = match key {
            "enter" | "return" => BindingCode::Enter,
            "up" => BindingCode::Up,
            "down" => BindingCode::Down,
            "left" => BindingCode::Left,
            "right" => BindingCode::Right,
            "home" => BindingCode::Home,
            "end" => BindingCode::End,
            "pageup" => BindingCode::PageUp,
            "pagedown" => BindingCode::PageDown,
            "tab" => BindingCode::Tab,
            "backtab" => BindingCode::BackTab,
            "backspace" => BindingCode::Backspace,
            "delete" | "del" => BindingCode::Delete,
            "esc" | "escape" => BindingCode::Esc,
            "space" => BindingCode::Char(' '),
            function
                if function
                    .strip_prefix('f')
                    .and_then(|number| number.parse::<u8>().ok())
                    .is_some_and(|number| (1..=24).contains(&number)) =>
            {
                BindingCode::Function(function[1..].parse().expect("validated function key"))
            }
            character if character.chars().count() == 1 => {
                BindingCode::Char(character.chars().next().expect("one character"))
            }
            _ => return Err(format!("快捷键 {raw:?} 的按键名称无效")),
        };
        Ok(Self { code, modifiers })
    }
}

impl Serialize for KeyBinding {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyBinding {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// 聊天编辑器命令键；导航键与 Shift 选择遵循终端通用约定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    pub submit: KeyBinding,
    pub newline: KeyBinding,
    pub history_previous: KeyBinding,
    pub history_next: KeyBinding,
    pub word_left: KeyBinding,
    pub word_right: KeyBinding,
    pub select_all: KeyBinding,
    pub copy: KeyBinding,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            submit: "enter".parse().expect("valid default key"),
            newline: "ctrl+o".parse().expect("valid default key"),
            history_previous: "ctrl+p".parse().expect("valid default key"),
            history_next: "ctrl+n".parse().expect("valid default key"),
            word_left: "ctrl+left".parse().expect("valid default key"),
            word_right: "ctrl+right".parse().expect("valid default key"),
            select_all: "ctrl+a".parse().expect("valid default key"),
            copy: "alt+c".parse().expect("valid default key"),
        }
    }
}

impl KeyBindings {
    pub fn validate(&self) -> Result<()> {
        let bindings = [
            ("submit", self.submit),
            ("newline", self.newline),
            ("history_previous", self.history_previous),
            ("history_next", self.history_next),
            ("word_left", self.word_left),
            ("word_right", self.word_right),
            ("select_all", self.select_all),
            ("copy", self.copy),
        ];
        let mut used = HashMap::new();
        for (action, binding) in bindings {
            if binding.intercepts_text_input() {
                bail!("快捷键 {action}={} 会拦截普通文本输入", binding);
            }
            if binding.is_reserved() {
                bail!("快捷键 {action}={} 与全局取消、弹窗或模型命令冲突", binding);
            }
            if let Some(existing) = used.insert(binding, action) {
                bail!("快捷键 {binding} 同时分配给 {existing} 和 {action}");
            }
        }
        Ok(())
    }
}

/// 全局代理类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Disabled,
    Http,
    Socks5,
}

impl ProxyMode {
    pub const ALL: [Self; 3] = [Self::Disabled, Self::Http, Self::Socks5];

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// 全局网络代理。URL 可包含代理认证信息，例如
/// `http://user:password@127.0.0.1:7890`。
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProxySettings {
    #[serde(default)]
    pub mode: ProxyMode,
    #[serde(default)]
    pub url: Option<String>,
}

impl fmt::Debug for ProxySettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxySettings")
            .field("mode", &self.mode)
            .field(
                "url",
                &self.url.as_deref().map(|url| redact_url(url, false)),
            )
            .finish()
    }
}

impl ProxySettings {
    pub fn normalize(&mut self) {
        self.url = self.url.as_deref().and_then(non_empty_trimmed);
    }

    /// 校验并解析运行时代理 URL。完整 `${ENV_VAR}` 引用只在这里解析，
    /// 不会写回 `self.url`。
    pub fn validated_url(&self) -> Result<Option<Url>, ProxyValidationError> {
        if self.mode == ProxyMode::Disabled {
            return Ok(None);
        }
        let raw = self
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or(ProxyValidationError::MissingUrl)?;
        let resolved =
            resolve_env_value(raw).map_err(|_| ProxyValidationError::EnvironmentUnavailable)?;
        self.validate_url(&resolved).map(Some)
    }

    /// 保存配置时只检查引用语法或字面 URL，不要求环境变量当前存在。
    fn validate_configured_url(&self) -> Result<(), ProxyValidationError> {
        if self.mode == ProxyMode::Disabled {
            return Ok(());
        }
        let raw = self
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or(ProxyValidationError::MissingUrl)?;
        if env_reference_name(raw).is_some() {
            return Ok(());
        }
        self.validate_url(raw).map(|_| ())
    }

    fn validate_url(&self, raw: &str) -> Result<Url, ProxyValidationError> {
        let url = Url::parse(raw).map_err(|_| ProxyValidationError::InvalidUrl)?;
        if url.host().is_none() {
            return Err(ProxyValidationError::MissingHost);
        }
        let valid_scheme = match self.mode {
            ProxyMode::Disabled => true,
            ProxyMode::Http => matches!(url.scheme(), "http" | "https"),
            ProxyMode::Socks5 => matches!(url.scheme(), "socks5" | "socks5h"),
        };
        if !valid_scheme {
            return Err(ProxyValidationError::SchemeMismatch);
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(ProxyValidationError::QueryOrFragment);
        }
        Ok(url)
    }
}

/// 可稳定映射到双语 UI 文案的代理校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyValidationError {
    MissingUrl,
    InvalidUrl,
    MissingHost,
    SchemeMismatch,
    QueryOrFragment,
    EnvironmentUnavailable,
}

impl fmt::Display for ProxyValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingUrl => "proxy URL is required",
            Self::InvalidUrl => "proxy URL is invalid",
            Self::MissingHost => "proxy URL must include a host",
            Self::SchemeMismatch => "proxy URL scheme does not match the selected proxy type",
            Self::QueryOrFragment => "proxy URL cannot contain a query string or fragment",
            Self::EnvironmentUnavailable => {
                "the environment variable referenced by the proxy is unavailable"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ProxyValidationError {}

/// MCP Server 初始化后得到的运行时能力快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpCapabilityMetadata {
    pub protocol_version: Option<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub resources: bool,
    pub prompts: bool,
    pub tools: bool,
    pub tool_count: usize,
}

impl McpCapabilityMetadata {
    pub(crate) fn normalize(&mut self) {
        self.protocol_version = bounded_optional(self.protocol_version.take(), 64);
        self.server_name = bounded_optional(self.server_name.take(), 128);
        self.server_version = bounded_optional(self.server_version.take(), 64);
    }
}

/// 一个远程 Streamable HTTP MCP Server。
///
/// `name` 来自 `[mcp_servers.<name>]` 的表键，不在表内重复写出。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    #[serde(skip)]
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub http_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_http_headers: BTreeMap<String, String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(
        default = "default_mcp_startup_timeout_seconds",
        skip_serializing_if = "is_default_mcp_startup_timeout_seconds"
    )]
    pub startup_timeout_sec: u64,
}

impl fmt::Debug for McpServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpServerConfig")
            .field("name", &self.name)
            .field("url", &redact_url(&self.url, true))
            .field("bearer_token_env_var", &self.bearer_token_env_var)
            .field(
                "http_header_names",
                &self.http_headers.keys().collect::<Vec<_>>(),
            )
            .field("env_http_headers", &self.env_http_headers)
            .field("enabled", &self.enabled)
            .field("startup_timeout_sec", &self.startup_timeout_sec)
            .finish()
    }
}

const fn default_mcp_startup_timeout_seconds() -> u64 {
    10
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_default_mcp_startup_timeout_seconds(value: &u64) -> bool {
    *value == default_mcp_startup_timeout_seconds()
}

impl McpServerConfig {
    pub fn new() -> Self {
        Self {
            name: "server".to_owned(),
            url: "https://example.com/mcp".to_owned(),
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            enabled: true,
            startup_timeout_sec: default_mcp_startup_timeout_seconds(),
        }
    }

    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        self.url = self.url.trim().to_owned();
        self.bearer_token_env_var = self
            .bearer_token_env_var
            .take()
            .as_deref()
            .and_then(non_empty_trimmed);
        self.http_headers = normalize_mcp_headers(std::mem::take(&mut self.http_headers));
        self.env_http_headers = normalize_mcp_headers(std::mem::take(&mut self.env_http_headers));
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > 128
            || self.name.chars().any(char::is_control)
        {
            bail!("MCP Server 名称不能为空且最多 128 字符");
        }
        if !(1..=3_600).contains(&self.startup_timeout_sec) {
            bail!("MCP Server {:?} 的启动超时必须位于 1..=3600 秒", self.name);
        }
        if self.url.len() > 8_192 {
            bail!("MCP Server {:?} 的 URL 超出长度限制", self.name);
        }
        let parsed = Url::parse(&self.url)
            .with_context(|| format!("MCP Server {:?} 的 URL 无效", self.name))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
            bail!(
                "MCP Server {:?} 的 URL 必须是有效的 HTTP(S) 地址",
                self.name
            );
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            bail!("MCP Server {:?} 的 URL 不能包含凭据", self.name);
        }
        if parsed.fragment().is_some() {
            bail!("MCP Server {:?} 的 URL 不能包含片段", self.name);
        }
        if self.http_headers.len() + self.env_http_headers.len() > 128 {
            bail!("MCP Server {:?} 的 Header 条目过多", self.name);
        }
        for (name, value) in &self.http_headers {
            validate_mcp_header(&self.name, name, value)?;
            if env_reference_name(value).is_some() {
                bail!(
                    "MCP Server {:?} 的 Header {name:?} 如需读取环境变量，必须改用 env_http_headers",
                    self.name
                );
            }
        }
        for (name, variable) in &self.env_http_headers {
            validate_mcp_header_name(&self.name, name)?;
            if !valid_env_name(variable) {
                bail!(
                    "MCP Server {:?} 的环境 Header {name:?} 必须引用有效的环境变量名",
                    self.name
                );
            }
            if self.http_headers.contains_key(name) {
                bail!(
                    "MCP Server {:?} 的 Header {name:?} 同时出现在 http_headers 和 env_http_headers",
                    self.name
                );
            }
        }
        if let Some(variable) = &self.bearer_token_env_var {
            if !valid_env_name(variable) {
                bail!(
                    "MCP Server {:?} 的 bearer_token_env_var 不是有效的环境变量名",
                    self.name
                );
            }
            if self.http_headers.contains_key("authorization")
                || self.env_http_headers.contains_key("authorization")
            {
                bail!(
                    "MCP Server {:?} 不能同时配置 bearer_token_env_var 和 Authorization Header",
                    self.name
                );
            }
        }
        Ok(())
    }
}

fn normalize_mcp_headers(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            (!name.is_empty() && !value.is_empty()).then_some((name, value))
        })
        .collect()
}

fn validate_mcp_header(server: &str, name: &str, value: &str) -> Result<()> {
    validate_mcp_header_name(server, name)?;
    if value.len() > 32_768 {
        bail!("MCP Server {server:?} 的 Header {name:?} 值超出长度限制");
    }
    reqwest::header::HeaderValue::from_str(value)
        .with_context(|| format!("MCP Server {server:?} 的 Header {name:?} 值无效"))?;
    Ok(())
}

fn validate_mcp_header_name(server: &str, name: &str) -> Result<()> {
    if name.len() > 256 {
        bail!("MCP Server {server:?} 的 Header {name:?} 名称超出长度限制");
    }
    let parsed_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
        .with_context(|| format!("MCP Server {server:?} 的 Header {name:?} 无效"))?;
    if matches!(
        parsed_name.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "accept"
            | "content-type"
            | "mcp-protocol-version"
            | "mcp-session-id"
            | "last-event-id"
    ) {
        bail!("MCP Server {server:?} 禁止覆盖 Header {name:?}");
    }
    Ok(())
}

fn serialize_mcp_servers<S>(
    servers: &Vec<McpServerConfig>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = serializer.serialize_map(Some(servers.len()))?;
    for server in servers {
        map.serialize_entry(&server.name, server)?;
    }
    map.end()
}

fn deserialize_mcp_servers<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<McpServerConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    struct McpServersVisitor;

    impl<'de> Visitor<'de> for McpServersVisitor {
        type Value = Vec<McpServerConfig>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a named MCP server table")
        }

        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut servers = Vec::with_capacity(map.size_hint().unwrap_or_default());
            while let Some((name, mut server)) = map.next_entry::<String, McpServerConfig>()? {
                server.name = name;
                servers.push(server);
            }
            Ok(servers)
        }
    }

    deserializer.deserialize_map(McpServersVisitor)
}

fn bounded_optional(value: Option<String>, max_chars: usize) -> Option<String> {
    value.as_deref().and_then(non_empty_trimmed).map(|value| {
        let mut bounded = value.chars().take(max_chars).collect::<String>();
        if value.chars().count() > max_chars {
            bounded.push('…');
        }
        bounded
    })
}

/// 一个 API 供应商：协议、地址、密钥与已选模型。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    /// 稳定标识；名称、地址和列表顺序改变后会话仍可解析到同一 Provider。
    #[serde(default)]
    pub id: String,
    /// 展示名；默认取 Base URL 的 hostname（去掉 `api.` 前缀），可自定义。
    pub name: String,
    /// 接口协议。
    pub api_kind: ApiKind,
    /// 自定义 API 版本根地址；`None` 使用所选协议的官方地址。
    pub base_url: Option<String>,
    /// API Key；`None` 时回退读取所选协议对应的环境变量。
    pub api_key: Option<SecretValue>,
    /// 从 Provider 模型目录同步后勾选保留的模型。
    pub models: Vec<String>,
    /// Provider 级静态 Header。Header 名统一规整为小写。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// 运行时模型目录元数据；不写入面向简单聊天客户端的用户配置。
    #[serde(default, skip_serializing)]
    pub model_settings: BTreeMap<String, ModelSettings>,
}

impl fmt::Debug for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Provider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("api_kind", &self.api_kind)
            .field(
                "base_url",
                &self.base_url.as_deref().map(|url| redact_url(url, false)),
            )
            .field("api_key", &self.api_key)
            .field("models", &self.models)
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("model_settings", &self.model_settings)
            .finish()
    }
}

impl Provider {
    pub fn new(api_kind: ApiKind) -> Self {
        Self {
            id: new_provider_id(),
            name: api_kind.default_name().to_owned(),
            api_kind,
            base_url: None,
            api_key: None,
            models: Vec::new(),
            headers: BTreeMap::new(),
            model_settings: BTreeMap::new(),
        }
    }

    /// 实际生效的 API Key：配置值优先，否则读协议默认环境变量。
    /// 配置中的完整 `${ENV_VAR}` 引用在运行时解析，原引用保持不变。
    pub fn resolved_api_key(&self) -> Result<Option<String>, crate::secret::SecretResolveError> {
        if let Some(value) = self
            .api_key
            .as_ref()
            .filter(|value| !value.expose().trim().is_empty())
        {
            return value.resolve().map(Some);
        }
        Ok(self.api_kind.api_key_envs().iter().find_map(|name| {
            std::env::var(name)
                .ok()
                .as_deref()
                .and_then(non_empty_trimmed)
        }))
    }

    pub fn has_auth_header(&self) -> bool {
        let expected = match self.api_kind {
            ApiKind::ChatCompletions | ApiKind::Responses => "authorization",
            ApiKind::AnthropicMessages => "x-api-key",
            ApiKind::GeminiGenerateContent => "x-goog-api-key",
        };
        self.headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case(expected) && !value.trim().is_empty())
    }

    /// 是否指向 OpenAI 官方（无自定义地址）。
    pub fn is_official(&self) -> bool {
        self.base_url
            .as_deref()
            .is_none_or(|url| url.trim().is_empty())
    }

    pub fn effective_base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| self.api_kind.default_base_url())
    }

    pub fn selection(&self, model: impl Into<String>) -> ModelSelection {
        ModelSelection {
            provider_id: self.id.clone(),
            provider_name: self.name.clone(),
            model: model.into(),
        }
    }

    pub fn settings_for_model(&self, model: &str) -> ModelSettings {
        self.model_settings.get(model).cloned().unwrap_or_default()
    }

    /// 清理用户可编辑字段，保持请求与 UI 使用同一份规范值。
    pub fn normalize(&mut self) {
        self.id = self.id.trim().to_owned();
        self.base_url = self
            .base_url
            .as_deref()
            .and_then(non_empty_trimmed)
            .map(|url| url.trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty());
        self.api_key = self
            .api_key
            .take()
            .and_then(|value| non_empty_trimmed(value.expose()))
            .map(SecretValue::from);
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            self.name = self
                .base_url
                .as_deref()
                .map(|url| name_from_url(Some(url)))
                .unwrap_or_else(|| self.api_kind.default_name().to_owned());
        }

        let mut models = Vec::with_capacity(self.models.len());
        for model in self.models.drain(..) {
            let model = model.trim();
            if !model.is_empty() && !models.iter().any(|existing| existing == model) {
                models.push(model.to_owned());
            }
        }
        self.models = models;

        let mut headers = BTreeMap::new();
        for (name, value) in std::mem::take(&mut self.headers) {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if !name.is_empty() && !value.is_empty() {
                headers.insert(name, value);
            }
        }
        self.headers = headers;

        let mut settings = BTreeMap::new();
        for (model, value) in std::mem::take(&mut self.model_settings) {
            let model = model.trim();
            if !model.is_empty() {
                settings.insert(model.to_owned(), value);
            }
        }
        self.model_settings = settings;
    }
}

fn new_provider_id() -> String {
    format!("provider-{}", Uuid::new_v4().simple())
}

fn validate_headers(provider: &Provider) -> Result<()> {
    for (name, value) in &provider.headers {
        let parsed_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("非法 Header 名 {name:?}"))?;
        reqwest::header::HeaderValue::from_str(value)
            .with_context(|| format!("Header {name:?} 的值包含非法字符"))?;
        if matches!(
            parsed_name.as_str(),
            "host" | "content-length" | "transfer-encoding" | "connection"
        ) {
            bail!("禁止覆盖传输层 Header {name:?}");
        }
    }
    Ok(())
}

fn validate_model_settings(api_kind: ApiKind, model: &str, settings: &ModelSettings) -> Result<()> {
    if model.trim().is_empty() {
        bail!("模型名称不能为空");
    }
    let params = &settings.parameters;
    if let Some(value) = params.temperature
        && (!value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        bail!("temperature 必须位于 0..=2");
    }
    if let Some(value) = params.top_p
        && (!value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        bail!("top_p 必须位于 0..=1");
    }
    if params.max_output_tokens == Some(0) {
        bail!("max_output_tokens 必须大于 0");
    }
    for (name, value) in [
        ("timeout_seconds", params.timeout_seconds),
        (
            "first_byte_timeout_seconds",
            params.first_byte_timeout_seconds,
        ),
        ("idle_timeout_seconds", params.idle_timeout_seconds),
    ] {
        if value.is_some_and(|seconds| seconds == 0 || seconds > 3_600) {
            bail!("{name} 必须位于 1..=3600");
        }
    }
    if params
        .retry_max_attempts
        .is_some_and(|attempts| !(1..=5).contains(&attempts))
    {
        bail!("retry_max_attempts 必须位于 1..=5");
    }
    match &params.reasoning {
        Some(ReasoningSettings::OpenAi { effort }) => {
            if !matches!(api_kind, ApiKind::ChatCompletions | ApiKind::Responses) {
                bail!("OpenAI reasoning 参数不能用于当前协议");
            }
            if !matches!(
                effort.as_str(),
                "minimal" | "low" | "medium" | "high" | "xhigh"
            ) {
                bail!("OpenAI reasoning effort 取值无效");
            }
        }
        Some(ReasoningSettings::Anthropic { budget_tokens }) => {
            if api_kind != ApiKind::AnthropicMessages {
                bail!("Anthropic thinking 参数不能用于当前协议");
            }
            if *budget_tokens == 0 {
                bail!("Anthropic thinking budget_tokens 必须大于 0");
            }
        }
        Some(ReasoningSettings::Gemini { thinking_budget }) => {
            if api_kind != ApiKind::GeminiGenerateContent {
                bail!("Gemini thinking 参数不能用于当前协议");
            }
            if *thinking_budget < -1 {
                bail!("Gemini thinking_budget 只能为 -1、0 或正数");
            }
        }
        None => {}
    }

    for key in params.extra_body.keys() {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            bail!("extra_body 的键不能为空");
        }
        if reserved_extra_body_key(api_kind, &normalized) {
            bail!("extra_body 禁止覆盖结构字段 {key:?}");
        }
    }
    Ok(())
}

fn reserved_extra_body_key(api_kind: ApiKind, key: &str) -> bool {
    let common = matches!(
        key,
        "model"
            | "stream"
            | "messages"
            | "input"
            | "contents"
            | "system_instruction"
            | "tools"
            | "tool_choice"
    );
    common
        || match api_kind {
            ApiKind::ChatCompletions => matches!(key, "stream_options"),
            ApiKind::Responses => matches!(key, "instructions"),
            ApiKind::AnthropicMessages => matches!(key, "system"),
            ApiKind::GeminiGenerateContent => matches!(key, "systeminstruction"),
        }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// 从 Base URL 提取展示名：`https://api.deepseek.com/v1` → `deepseek.com`
/// （仅去掉 `api.` 前缀与端口/路径）。
pub fn name_from_url(base_url: Option<&str>) -> String {
    let Some(url) = base_url.map(str::trim).filter(|u| !u.is_empty()) else {
        return "OpenAI".to_owned();
    };
    let host = Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .or_else(|| {
            Url::parse(&format!("http://{url}"))
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
        })
        .unwrap_or_default();
    let name = host.strip_prefix("api.").unwrap_or(&host);
    if name.is_empty() {
        "OpenAI".to_owned()
    } else {
        name.to_owned()
    }
}

/// 应用设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 配置结构版本，用于识别并隔离不兼容配置。
    #[serde(default)]
    pub config_version: u32,
    /// 全部供应商。
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// 当前用于对话的供应商下标。
    #[serde(default)]
    pub current_provider: usize,
    /// 当前使用的模型。
    #[serde(default = "default_model")]
    pub model: String,
    /// 界面语言。
    #[serde(default)]
    pub language: Lang,
    /// 视觉主题。
    #[serde(default)]
    pub theme: Theme,
    /// 所有 LLM 与模型同步请求共用的代理。
    #[serde(default)]
    pub proxy: ProxySettings,
    /// 上下文预算与压缩策略。
    #[serde(default, skip_serializing_if = "is_default")]
    pub context: ContextSettings,
    /// 独立持久化的 MCP Server；运行状态由 MCP Registry 管理。
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_mcp_servers",
        deserialize_with = "deserialize_mcp_servers"
    )]
    pub mcp_servers: Vec<McpServerConfig>,
    /// 聊天编辑器可配置命令键。
    #[serde(default, skip_serializing_if = "is_default")]
    pub keybindings: KeyBindings,
    /// 是否在聊天区显示 BT-7274 像素画背景（默认关闭，后续再默认开启）。
    #[serde(default = "default_false")]
    pub show_titan: bool,
}

fn default_model() -> String {
    "gpt-4o-mini".to_owned()
}

fn default_false() -> bool {
    false
}

fn is_default<T>(value: &T) -> bool
where
    T: Default + PartialEq,
{
    value == &T::default()
}

impl Default for Settings {
    fn default() -> Self {
        let mut provider = Provider::new(ApiKind::default());
        provider.id = "provider-openai-default".to_owned();
        provider.models = vec![default_model()];
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            providers: vec![provider],
            current_provider: 0,
            model: default_model(),
            language: Lang::default(),
            theme: Theme::default(),
            proxy: ProxySettings::default(),
            context: ContextSettings::default(),
            mcp_servers: Vec::new(),
            keybindings: KeyBindings::default(),
            show_titan: default_false(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigVersionIssue {
    Missing,
    Invalid,
    Unsupported(u32),
}

impl ConfigVersionIssue {
    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::Unsupported(_) => "unsupported",
        }
    }

    fn declared(self) -> Option<u32> {
        match self {
            Self::Unsupported(version) => Some(version),
            Self::Missing | Self::Invalid => None,
        }
    }
}

impl Settings {
    /// 当前精简配置不提供旧版本迁移。
    fn ensure_current_version(&self) -> Result<()> {
        if self.config_version != CURRENT_CONFIG_VERSION {
            bail!(
                "配置版本 {} 与当前程序要求的版本 {} 不一致",
                self.config_version,
                CURRENT_CONFIG_VERSION
            );
        }
        Ok(())
    }

    /// 空配置补齐一个默认 Provider。
    fn ensure_provider(&mut self) {
        if self.providers.is_empty() {
            let mut provider = Provider::new(ApiKind::default());
            provider.models = vec![self.model.clone()];
            self.providers = vec![provider];
        }
    }

    fn assign_provider_ids(&mut self) {
        let mut used = std::collections::HashSet::new();
        for (index, provider) in self.providers.iter_mut().enumerate() {
            let id = provider.id.trim();
            if id.is_empty() || !used.insert(id.to_owned()) {
                let base = format!("provider-migrated-{}", index + 1);
                let mut candidate = base.clone();
                let mut suffix = 2;
                while used.contains(&candidate) {
                    candidate = format!("{base}-{suffix}");
                    suffix += 1;
                }
                provider.id = candidate;
                used.insert(provider.id.clone());
            }
        }
    }

    /// 规整配置：清理字段、钳制下标并兜底当前模型。
    pub fn normalize(&mut self) {
        self.model = self.model.trim().to_owned();
        self.proxy.normalize();
        self.context.normalize();
        for server in &mut self.mcp_servers {
            server.normalize();
        }
        self.ensure_provider();
        for provider in &mut self.providers {
            provider.normalize();
        }
        self.assign_provider_ids();
        // 删除供应商后下标可能越界
        self.current_provider = self.current_provider.min(self.providers.len() - 1);
        let provider = &self.providers[self.current_provider];
        if provider.models.is_empty() {
            // 模型列表为空时保底默认模型，避免对话请求无模型可用
            if self.model.trim().is_empty() {
                self.model = default_model();
            }
        } else if provider.models.iter().all(|m| *m != self.model) {
            self.model = provider.models[0].clone();
        }
    }

    /// 保存前需要满足的跨字段约束。
    pub fn validate(&self) -> Result<(), ProxyValidationError> {
        self.proxy.validate_configured_url()?;
        Ok(())
    }

    /// 加载与保存时执行的结构校验。UI 内的代理即时校验仍由 `validate` 提供。
    fn validate_document(&self) -> Result<()> {
        self.validate().context("代理配置无效")?;
        if self.config_version != CURRENT_CONFIG_VERSION {
            bail!(
                "配置版本 {} 未迁移到当前版本 {}",
                self.config_version,
                CURRENT_CONFIG_VERSION
            );
        }
        if self.providers.is_empty() {
            bail!("配置至少需要一个供应商");
        }
        if self.model.trim().is_empty() {
            bail!("当前模型名称不能为空");
        }
        self.keybindings.validate().context("快捷键配置无效")?;
        self.validate_mcp_servers()?;
        for (index, provider) in self.providers.iter().enumerate() {
            if provider.id.trim().is_empty() {
                bail!("第 {} 个供应商缺少稳定 id", index + 1);
            }
            validate_headers(provider)
                .with_context(|| format!("第 {} 个供应商的自定义 Header 无效", index + 1))?;
            for (model, settings) in &provider.model_settings {
                validate_model_settings(provider.api_kind, model, settings)
                    .with_context(|| format!("第 {} 个供应商模型 {model} 的参数无效", index + 1))?;
            }
            let Some(raw_url) = provider.base_url.as_deref() else {
                continue;
            };
            if env_reference_name(raw_url).is_some() {
                continue;
            }
            let url = Url::parse(raw_url)
                .with_context(|| format!("第 {} 个供应商的 Base URL 无效", index + 1))?;
            if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
                bail!(
                    "第 {} 个供应商的 Base URL 必须是有效的 HTTP(S) 地址",
                    index + 1
                );
            }
            if url.query().is_some() || url.fragment().is_some() {
                bail!(
                    "第 {} 个供应商的 Base URL 不能包含查询参数或片段",
                    index + 1
                );
            }
        }
        Ok(())
    }

    /// MCP 配置的独立结构校验，供设置 UI 在关闭草稿前给出就地错误。
    pub fn validate_mcp_servers(&self) -> Result<()> {
        if self.mcp_servers.len() > 64 {
            bail!("MCP Server 最多配置 64 个");
        }
        let mut names = HashSet::new();
        for (index, server) in self.mcp_servers.iter().enumerate() {
            server
                .validate()
                .with_context(|| format!("第 {} 个 MCP Server 配置无效", index + 1))?;
            if !names.insert(server.name.as_str()) {
                bail!("MCP Server 名称 {:?} 重复", server.name);
            }
        }
        Ok(())
    }

    /// 从当前版本 TOML 文本读取、规整并校验配置。
    fn from_toml(raw: &str) -> Result<Self> {
        let document: toml::Value =
            toml::from_str(raw).map_err(|_| color_eyre::eyre::eyre!("配置 TOML 语法无效"))?;
        let declared_version = match document.get("config_version") {
            None => bail!("配置缺少 config_version"),
            Some(toml::Value::Integer(version)) if *version >= 0 => {
                u32::try_from(*version).context("config_version 超出支持的整数范围")?
            }
            Some(_) => bail!("config_version 必须是非负整数"),
        };
        if declared_version != CURRENT_CONFIG_VERSION {
            bail!(
                "配置版本 {declared_version} 与当前程序要求的版本 {} 不一致",
                CURRENT_CONFIG_VERSION
            );
        }

        let mut settings: Self =
            toml::from_str(raw).map_err(|_| color_eyre::eyre::eyre!("配置字段类型或取值无效"))?;
        settings.ensure_current_version()?;
        settings.normalize();
        settings.validate_document()?;
        Ok(settings)
    }

    /// 当前用于对话的供应商。
    pub fn provider(&self) -> &Provider {
        &self.providers[self.current_provider.min(self.providers.len() - 1)]
    }

    pub fn provider_by_id(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    pub fn selection_available(&self, selection: &ModelSelection) -> bool {
        self.provider_by_id(&selection.provider_id)
            .is_some_and(|provider| {
                !selection.model.trim().is_empty()
                    && (provider.models.is_empty() || provider.models.contains(&selection.model))
            })
    }

    pub fn default_selection(&self) -> ModelSelection {
        self.provider().selection(self.model.clone())
    }

    /// 远程 MCP Runtime 使用的配置快照。
    pub fn runtime_mcp_servers(&self) -> Vec<McpServerConfig> {
        self.mcp_servers.clone()
    }

    /// 配置文件路径。
    pub fn config_path() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("无法定位系统配置目录")?
            .join(env!("CARGO_PKG_NAME"))
            .join("config.toml"))
    }

    /// 加载配置；文件不存在时返回默认值。
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;

        // 版本不匹配的配置不做迁移。先原样保留，再以当前默认配置启动，
        // 避免一次范围收缩让整个 TUI 永久卡在启动边界。
        if let Some(issue) = config_version_issue(&raw) {
            let backup = backup_incompatible_config(path)?;
            tracing::warn!(
                path = %path.display(),
                backup = %backup.display(),
                reason = issue.label(),
                declared_version = ?issue.declared(),
                required_version = CURRENT_CONFIG_VERSION,
                "配置版本不兼容；已原样备份并使用默认配置启动"
            );
            return Ok(Self::default());
        }

        Self::from_toml(&raw).with_context(|| format!("加载配置失败: {}", path.display()))
    }

    /// 保存配置，必要时创建父目录。
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
        let mut normalized = self.clone();
        normalized
            .ensure_current_version()
            .context("配置版本检查失败")?;
        normalized.normalize();
        normalized.validate_document()?;
        let raw = toml::to_string_pretty(&normalized).context("序列化配置失败")?;
        atomic_write_private(&path, raw.as_bytes())
            .with_context(|| format!("写入配置失败: {}", path.display()))?;
        Ok(())
    }
}

fn config_version_issue(raw: &str) -> Option<ConfigVersionIssue> {
    let document: toml::Value = toml::from_str(raw).ok()?;
    match document.get("config_version") {
        None => Some(ConfigVersionIssue::Missing),
        Some(toml::Value::Integer(version)) if *version >= 0 => {
            let Ok(version) = u32::try_from(*version) else {
                return Some(ConfigVersionIssue::Invalid);
            };
            (version != CURRENT_CONFIG_VERSION).then_some(ConfigVersionIssue::Unsupported(version))
        }
        Some(_) => Some(ConfigVersionIssue::Invalid),
    }
}

fn backup_incompatible_config(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    let backup = path.with_file_name(format!(
        "{file_name}.incompatible-{}.bak",
        Uuid::new_v4().simple()
    ));
    std::fs::rename(path, &backup).with_context(|| {
        format!(
            "备份不兼容配置失败: {} -> {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_from_url_common_cases() {
        assert_eq!(name_from_url(None), "OpenAI");
        assert_eq!(
            name_from_url(Some("https://api.deepseek.com/v1")),
            "deepseek.com"
        );
        assert_eq!(
            name_from_url(Some("http://localhost:11434/v1")),
            "localhost"
        );
    }

    #[test]
    fn defaults_are_chat_focused() {
        let settings = Settings::default();
        assert_eq!(settings.config_version, CURRENT_CONFIG_VERSION);
        assert_eq!(settings.providers.len(), 1);
        assert_eq!(settings.provider().name, "OpenAI");
        assert_eq!(settings.model, "gpt-4o-mini");
        assert_eq!(settings.theme, Theme::Vanguard);
        assert_eq!(settings.proxy, ProxySettings::default());
        assert!(settings.mcp_servers.is_empty());
        assert!(!settings.show_titan);
    }

    #[test]
    fn settings_example_matches_the_current_schema() {
        let settings = Settings::from_toml(include_str!("../settings.example.toml"))
            .expect("settings.example.toml must remain a valid current configuration");

        assert_eq!(settings.config_version, CURRENT_CONFIG_VERSION);
        assert_eq!(settings.providers.len(), 1);
        assert_eq!(settings.provider().api_kind, ApiKind::GeminiGenerateContent);
        assert!(settings.provider().model_settings.is_empty());
        assert_eq!(settings.model, "gemini-2.5-flash");
        assert_eq!(settings.context, ContextSettings::default());
        assert_eq!(settings.keybindings, KeyBindings::default());
        assert!(settings.mcp_servers.is_empty());

        let mut serializable = settings.clone();
        serializable.providers[0].model_settings.insert(
            "gemini-2.5-flash".to_owned(),
            ModelSettings {
                parameters: ModelParameters {
                    temperature: Some(0.7),
                    ..ModelParameters::default()
                },
                ..ModelSettings::default()
            },
        );
        let saved = toml::to_string_pretty(&serializable).unwrap();
        assert!(!saved.contains("[context]"));
        assert!(!saved.contains("[keybindings]"));
        assert!(!saved.contains("[providers.headers]"));
        assert!(!saved.contains("[providers.model_settings]"));
        assert!(!saved.contains("[mcp_servers.capabilities]"));
    }

    #[test]
    fn current_shape_roundtrips_without_migration() {
        let mut settings = Settings::default();
        settings.providers[0].api_key = Some(SecretValue::from("${OPENAI_API_KEY}"));
        settings.mcp_servers.push(McpServerConfig {
            name: "docs".to_owned(),
            url: "https://mcp.example.com/mcp".to_owned(),
            bearer_token_env_var: Some("MCP_TOKEN".to_owned()),
            http_headers: BTreeMap::from([("x-tenant".to_owned(), "example".to_owned())]),
            env_http_headers: BTreeMap::from([("x-api-key".to_owned(), "MCP_API_KEY".to_owned())]),
            enabled: true,
            startup_timeout_sec: 30,
        });
        let raw = toml::to_string_pretty(&settings).unwrap();
        let parsed = Settings::from_toml(&raw).unwrap();
        assert_eq!(parsed, settings);
        assert!(raw.contains("[mcp_servers.docs]"));
        assert!(raw.contains("bearer_token_env_var = \"MCP_TOKEN\""));
        assert!(raw.contains("startup_timeout_sec = 30"));
        assert!(!raw.contains("[[mcp_servers]]"));
        assert!(!raw.contains("transport"));
        assert!(!raw.contains("[context]"));
        assert!(!raw.contains("[keybindings]"));
        assert!(!raw.contains("[providers.headers]"));
        assert!(!raw.contains("[providers.model_settings]"));
    }

    #[test]
    fn mcp_defaults_and_environment_sources_follow_codex_shape() {
        let mut settings = Settings::default();
        let mut server = McpServerConfig::new();
        server.name = "docs".to_owned();
        settings.mcp_servers.push(server.clone());

        let raw = toml::to_string_pretty(&settings).unwrap();
        assert!(raw.contains("[mcp_servers.docs]"));
        assert!(!raw.contains("startup_timeout_sec"));
        assert!(!raw.contains("enabled ="));

        server.bearer_token_env_var = Some("${MCP_TOKEN}".to_owned());
        server.normalize();
        assert!(server.validate().is_err());

        server.bearer_token_env_var = None;
        server
            .http_headers
            .insert("x-api-key".to_owned(), "${MCP_API_KEY}".to_owned());
        server.normalize();
        assert!(server.validate().is_err());

        server.http_headers.clear();
        server
            .env_http_headers
            .insert("x-api-key".to_owned(), "MCP_API_KEY".to_owned());
        assert!(server.validate().is_ok());

        server.startup_timeout_sec = 0;
        server.normalize();
        assert!(server.validate().is_err());
    }

    #[test]
    fn old_or_future_config_versions_are_rejected() {
        for version in [CURRENT_CONFIG_VERSION - 1, CURRENT_CONFIG_VERSION + 1] {
            let raw = toml::to_string_pretty(&Settings {
                config_version: version,
                ..Settings::default()
            })
            .unwrap();
            assert!(Settings::from_toml(&raw).is_err());
        }
        let mut raw = toml::to_string_pretty(&Settings::default()).unwrap();
        raw = raw
            .lines()
            .filter(|line| !line.starts_with("config_version"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(Settings::from_toml(&raw).is_err());
    }

    #[test]
    fn incompatible_config_is_backed_up_before_default_startup() {
        let root = std::env::temp_dir().join(format!(
            "bt-7274-incompatible-config-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let mut raw = toml::to_string_pretty(&Settings::default()).unwrap();
        raw = raw
            .lines()
            .filter(|line| !line.starts_with("config_version"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &raw).unwrap();

        let loaded = Settings::load_from_path(&path).unwrap();

        assert_eq!(loaded, Settings::default());
        assert!(!path.exists());
        let backups = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert!(
            backups[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config.toml.incompatible-")
        );
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), raw);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_and_stdio_mcp_shapes_are_rejected() {
        let legacy = format!(
            r#"
config_version = {CURRENT_CONFIG_VERSION}

[[mcp_servers]]
id = "docs"
name = "Documentation"

[mcp_servers.transport]
kind = "streamable_http"
url = "https://mcp.example.com/mcp"
"#
        );
        assert!(Settings::from_toml(&legacy).is_err());

        let duplicated_name = format!(
            r#"
config_version = {CURRENT_CONFIG_VERSION}

[mcp_servers.docs]
name = "Documentation"
url = "https://mcp.example.com/mcp"
"#
        );
        assert!(Settings::from_toml(&duplicated_name).is_err());

        let stdio = r#"
command = "node"
args = []
"#;
        assert!(toml::from_str::<McpServerConfig>(stdio).is_err());
    }

    #[test]
    fn mcp_http_validation_rejects_credentials_and_duplicate_names() {
        let mut settings = Settings::default();
        let server = McpServerConfig {
            name: "docs".to_owned(),
            url: "https://user:secret@mcp.example.com/mcp".to_owned(),
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            enabled: true,
            startup_timeout_sec: 30,
        };
        settings.mcp_servers.push(server);
        assert!(settings.validate_mcp_servers().is_err());

        let server = McpServerConfig {
            name: "docs".to_owned(),
            url: "https://mcp.example.com/mcp".to_owned(),
            bearer_token_env_var: None,
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            enabled: true,
            startup_timeout_sec: 30,
        };
        settings.mcp_servers = vec![server.clone(), server];
        assert!(settings.validate_mcp_servers().is_err());
    }

    #[test]
    fn proxy_validation_matches_selected_mode() {
        let mut proxy = ProxySettings {
            mode: ProxyMode::Http,
            url: Some("http://127.0.0.1:7890".to_owned()),
        };
        assert!(proxy.validated_url().unwrap().is_some());
        proxy.mode = ProxyMode::Socks5;
        proxy.url = Some("socks5://127.0.0.1:1080".to_owned());
        assert!(proxy.validated_url().unwrap().is_some());
        proxy.url = Some("http://127.0.0.1:1080".to_owned());
        assert!(proxy.validated_url().is_err());
    }

    #[test]
    fn configuration_debug_output_omits_credentials() {
        let mut settings = Settings::default();
        settings.providers[0].api_key = Some(SecretValue::from("provider-secret"));
        settings.mcp_servers.push(McpServerConfig {
            name: "docs".to_owned(),
            url: "https://mcp.example.com/mcp?token=query-secret".to_owned(),
            bearer_token_env_var: None,
            http_headers: BTreeMap::from([
                ("x-api-key".to_owned(), "header-secret".to_owned()),
                ("authorization".to_owned(), "bearer-secret".to_owned()),
            ]),
            env_http_headers: BTreeMap::new(),
            enabled: true,
            startup_timeout_sec: 30,
        });
        let debug = format!("{settings:?}");
        for secret in [
            "provider-secret",
            "query-secret",
            "header-secret",
            "bearer-secret",
        ] {
            assert!(!debug.contains(secret));
        }
    }
}
