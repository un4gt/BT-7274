//! 应用状态机：焦点、输入、会话切换、流式生成、多供应商设置与模型切换。

use crate::clipboard;
use crate::config::{
    ApiKind, KeyBinding, KeyBindings, McpServerConfig, ModelSelection, ModelSettings, Provider,
    ProxySettings, Settings, name_from_url,
};
use crate::editor::ChatEditor;
use crate::event::{AppEvent, Event, EventHandler};
use crate::i18n::Lang;
use crate::runtime::{
    context::{
        CompactionPlan, ContextBudget, ContextInputs, build_budget, prepare_compaction,
        undo_last_compaction,
    },
    conversation::{IncompleteRecovery, Message, MessageStatus, Role, Session},
    error::{RuntimeError, RuntimeErrorKind},
    mcp::{McpRegistry, McpRuntimeEvent, McpServerSnapshot, McpServerStatus},
    model::{
        self as llm, ModelCatalog, ModelStreamEvent, RequestFeatures, RequestMetrics, StopReason,
        Usage,
    },
    task::{CancellationReason, CancellationToken},
};
use crate::text::normalize_newlines;
use crate::ui::mouse::{MouseMap, MouseTarget};
use crate::ui::{self, markdown::MarkdownCodeBlock, sparkle::Sparkle};
use crate::ui::{activity::ActivityOverlay, transcript::ChatViewState};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{DefaultTerminal, layout::Position};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;
use tokio::task::JoinHandle;
use unicode_width::UnicodeWidthStr;

/// 当前键盘焦点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Input,
    Sidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub level: NoticeLevel,
    pub summary: String,
    pub error: Option<RuntimeError>,
}

impl Notice {
    fn info(summary: String) -> Self {
        Self {
            level: NoticeLevel::Info,
            summary,
            error: None,
        }
    }

    fn warning(summary: String) -> Self {
        Self {
            level: NoticeLevel::Warning,
            summary,
            error: None,
        }
    }

    fn error(summary: String, error: RuntimeError) -> Self {
        Self {
            level: NoticeLevel::Error,
            summary,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErrorDetailState {
    pub session_id: String,
    pub scroll: u16,
}

#[derive(Debug, Clone)]
pub struct CodeBlockOverlayState {
    pub blocks: Vec<MarkdownCodeBlock>,
    pub selected: usize,
    pub vertical_scroll: u16,
    pub horizontal_scroll: usize,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryChoice {
    Continue,
    Keep,
    Discard,
}

impl RecoveryChoice {
    pub const ALL: [Self; 3] = [Self::Continue, Self::Keep, Self::Discard];
}

#[derive(Debug)]
pub struct RecoveryState {
    pub session_id: String,
    pub message_index: usize,
    pub selected: usize,
    pub error: Option<String>,
}

/// 会话级弹窗。任何时刻只允许一个，避免焦点与 Esc 行为冲突。
#[derive(Debug)]
pub enum ConversationOverlay {
    Rename {
        session_id: String,
        edit: LineEdit,
        error: Option<String>,
    },
    Search {
        edit: LineEdit,
        selected: usize,
    },
    Recovery(RecoveryState),
    Compact {
        session_id: String,
        plan: CompactionPlan,
        scroll: u16,
    },
}

/// 设置弹窗「外观」分类中的字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Language,
    Theme,
    TitanArt,
    Whimsy,
}

impl SettingsField {
    pub const ALL: [Self; 4] = [Self::Language, Self::Theme, Self::TitanArt, Self::Whimsy];

    fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }
}

/// 设置弹窗左侧的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsCategory {
    Models,
    Context,
    Mcp,
    Network,
    Keyboard,
    Appearance,
}

impl SettingsCategory {
    pub const ALL: [Self; 6] = [
        Self::Models,
        Self::Context,
        Self::Mcp,
        Self::Network,
        Self::Keyboard,
        Self::Appearance,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// 设置弹窗「键盘」分类中的可配置命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindingField {
    Submit,
    Newline,
    HistoryPrevious,
    HistoryNext,
    WordLeft,
    WordRight,
    SelectAll,
    Copy,
}

impl KeybindingField {
    pub const ALL: [Self; 8] = [
        Self::Submit,
        Self::Newline,
        Self::HistoryPrevious,
        Self::HistoryNext,
        Self::WordLeft,
        Self::WordRight,
        Self::SelectAll,
        Self::Copy,
    ];

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    pub fn binding(self, bindings: &KeyBindings) -> KeyBinding {
        match self {
            Self::Submit => bindings.submit,
            Self::Newline => bindings.newline,
            Self::HistoryPrevious => bindings.history_previous,
            Self::HistoryNext => bindings.history_next,
            Self::WordLeft => bindings.word_left,
            Self::WordRight => bindings.word_right,
            Self::SelectAll => bindings.select_all,
            Self::Copy => bindings.copy,
        }
    }

    fn set(self, bindings: &mut KeyBindings, binding: KeyBinding) {
        match self {
            Self::Submit => bindings.submit = binding,
            Self::Newline => bindings.newline = binding,
            Self::HistoryPrevious => bindings.history_previous = binding,
            Self::HistoryNext => bindings.history_next = binding,
            Self::WordLeft => bindings.word_left = binding,
            Self::WordRight => bindings.word_right = binding,
            Self::SelectAll => bindings.select_all = binding,
            Self::Copy => bindings.copy = binding,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextField {
    AutoCompact,
    Threshold,
    ReservedOutput,
}

impl ContextField {
    pub const ALL: [Self; 3] = [Self::AutoCompact, Self::Threshold, Self::ReservedOutput];

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }
}

/// 设置弹窗「网络」分类中的字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkField {
    ProxyMode,
    ProxyUrl,
}

impl NetworkField {
    pub const ALL: [Self; 2] = [Self::ProxyMode, Self::ProxyUrl];

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }
}

/// 设置弹窗当前焦点栏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPane {
    Category,
    Content,
}

/// 「模型」分类内的两个分区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsSection {
    Providers,
    ModelsList,
}

/// 单行文本编辑缓冲（向导字段 / 手动添加模型行）。
#[derive(Debug, Default)]
pub struct LineEdit {
    pub buffer: Vec<char>,
    pub cursor: usize,
}

impl LineEdit {
    pub fn from_text(text: &str) -> Self {
        let buffer: Vec<char> = text.chars().collect();
        let cursor = buffer.len();
        Self { buffer, cursor }
    }

    pub fn text(&self) -> String {
        self.buffer.iter().collect()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    fn insert_text(&mut self, text: &str) {
        insert_text(&mut self.buffer, &mut self.cursor, text, false);
    }

    /// 处理可打印输入键；返回是否消费了该键。
    pub fn accept_key(&mut self, key_event: KeyEvent) -> bool {
        let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        match key_event.code {
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.buffer.remove(self.cursor);
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.buffer.len() {
                    self.buffer.remove(self.cursor);
                }
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.buffer.len());
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.cursor = self.buffer.len();
                true
            }
            KeyCode::Char('u' | 'U') if ctrl => {
                self.clear();
                true
            }
            KeyCode::Char(c) if plain => {
                self.buffer.insert(self.cursor, c);
                self.cursor += 1;
                true
            }
            _ => false,
        }
    }
}

/// 新增供应商向导：选协议 → 填连接 → 同步并勾选模型。
#[derive(Debug)]
pub struct ProviderWizard {
    /// 同步任务代号，用于配对 `ModelsSynced` 事件。
    pub id: u64,
    /// `Some(index)` 表示保存时替换该供应商，而非新增。
    pub edit_index: Option<usize>,
    /// 1 选协议 / 2 连接配置 / 3 选择模型。
    pub step: u8,
    /// step1 选中项，对应 [`ApiKind::ALL`]。
    pub kind_pos: usize,
    pub provider_id: String,
    pub name: LineEdit,
    pub url: LineEdit,
    pub key: LineEdit,
    /// JSON object 形式的 Provider 静态 Header。
    pub headers: LineEdit,
    /// step2 焦点字段：0 名称 / 1 Base URL / 2 API Key / 3 Headers。
    pub field: usize,
    pub fetching: bool,
    pub fetched: Option<Vec<String>>,
    pub fetched_settings: BTreeMap<String, ModelSettings>,
    pub selected: Vec<bool>,
    pub list_pos: usize,
    pub error: Option<String>,
    /// 「至少选 1 个模型」的提示闪现。
    pub warned: bool,
    /// step3 底部手动添加行（同步失败或不给列表的服务用）。
    pub manual: LineEdit,
    pub manual_active: bool,
}

impl ProviderWizard {
    fn new(id: u64) -> Self {
        let provider = Provider::new(ApiKind::default());
        Self {
            id,
            edit_index: None,
            step: 1,
            kind_pos: 0,
            provider_id: provider.id,
            name: LineEdit::default(),
            url: LineEdit::default(),
            key: LineEdit::default(),
            headers: LineEdit::default(),
            field: 0,
            fetching: false,
            fetched: None,
            fetched_settings: BTreeMap::new(),
            selected: Vec::new(),
            list_pos: 0,
            error: None,
            warned: false,
            manual: LineEdit::default(),
            manual_active: false,
        }
    }

    fn editing(id: u64, index: usize, provider: &Provider) -> Self {
        Self {
            id,
            edit_index: Some(index),
            step: 1,
            kind_pos: provider.api_kind.index(),
            provider_id: provider.id.clone(),
            name: LineEdit::from_text(&provider.name),
            url: LineEdit::from_text(provider.base_url.as_deref().unwrap_or_default()),
            key: LineEdit::from_text(provider.api_key.as_deref().unwrap_or_default()),
            headers: LineEdit::from_text(
                &serde_json::to_string(&provider.headers).unwrap_or_default(),
            ),
            field: 0,
            fetching: false,
            fetched: Some(provider.models.clone()),
            fetched_settings: provider.model_settings.clone(),
            selected: vec![true; provider.models.len()],
            list_pos: 0,
            error: None,
            warned: false,
            manual: LineEdit::default(),
            manual_active: false,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.edit_index.is_some()
    }

    pub fn api_kind(&self) -> ApiKind {
        ApiKind::ALL[self.kind_pos.min(ApiKind::ALL.len() - 1)]
    }

    pub fn line(&self, field: usize) -> &LineEdit {
        match field {
            0 => &self.name,
            1 => &self.url,
            2 => &self.key,
            _ => &self.headers,
        }
    }

    fn line_mut(&mut self, field: usize) -> &mut LineEdit {
        match field {
            0 => &mut self.name,
            1 => &mut self.url,
            2 => &mut self.key,
            _ => &mut self.headers,
        }
    }
}

/// 远程 MCP Server 新增/编辑向导。
#[derive(Debug)]
pub struct McpServerWizard {
    pub edit_index: Option<usize>,
    /// 1 表示打开，0 表示已提交或取消。
    pub step: u8,
    pub field: usize,
    pub name: LineEdit,
    pub url: LineEdit,
    pub http_headers: LineEdit,
    pub env_http_headers: LineEdit,
    pub bearer_token_env_var: LineEdit,
    pub timeout: LineEdit,
    enabled: bool,
    pub error: Option<String>,
}

impl McpServerWizard {
    fn new() -> Self {
        let config = McpServerConfig::new();
        Self {
            edit_index: None,
            step: 1,
            field: 0,
            name: LineEdit::from_text(&config.name),
            url: LineEdit::from_text(&config.url),
            http_headers: LineEdit::from_text("{}"),
            env_http_headers: LineEdit::from_text("{}"),
            bearer_token_env_var: LineEdit::default(),
            timeout: LineEdit::from_text(&config.startup_timeout_sec.to_string()),
            enabled: true,
            error: None,
        }
    }

    fn editing(index: usize, config: &McpServerConfig) -> Self {
        let mut wizard = Self::new();
        wizard.edit_index = Some(index);
        wizard.name = LineEdit::from_text(&config.name);
        wizard.url = LineEdit::from_text(&config.url);
        wizard.http_headers = LineEdit::from_text(
            &serde_json::to_string(&config.http_headers).unwrap_or_else(|_| "{}".to_owned()),
        );
        wizard.env_http_headers = LineEdit::from_text(
            &serde_json::to_string(&config.env_http_headers).unwrap_or_else(|_| "{}".to_owned()),
        );
        wizard.bearer_token_env_var =
            LineEdit::from_text(config.bearer_token_env_var.as_deref().unwrap_or_default());
        wizard.timeout = LineEdit::from_text(&config.startup_timeout_sec.to_string());
        wizard.enabled = config.enabled;
        wizard
    }

    pub fn is_editing(&self) -> bool {
        self.edit_index.is_some()
    }

    pub fn field_count(&self) -> usize {
        6
    }

    pub fn line(&self, field: usize) -> &LineEdit {
        match field {
            0 => &self.name,
            1 => &self.url,
            2 => &self.http_headers,
            3 => &self.env_http_headers,
            4 => &self.bearer_token_env_var,
            _ => &self.timeout,
        }
    }

    fn line_mut(&mut self, field: usize) -> &mut LineEdit {
        match field {
            0 => &mut self.name,
            1 => &mut self.url,
            2 => &mut self.http_headers,
            3 => &mut self.env_http_headers,
            4 => &mut self.bearer_token_env_var,
            _ => &mut self.timeout,
        }
    }

    pub fn sensitive_field(&self, field: usize) -> bool {
        field == 2
    }

    fn build_config(&self) -> Result<McpServerConfig, String> {
        let startup_timeout_sec = self
            .timeout
            .text()
            .trim()
            .parse::<u64>()
            .map_err(|_| "MCP timeout must be an integer number of seconds".to_owned())?;
        let http_headers = parse_json_or_default::<BTreeMap<String, String>>(
            &self.http_headers.text(),
            BTreeMap::new(),
        )
        .map_err(|error| format!("Invalid HTTP Headers JSON: {error}"))?;
        let env_http_headers = parse_json_or_default::<BTreeMap<String, String>>(
            &self.env_http_headers.text(),
            BTreeMap::new(),
        )
        .map_err(|error| format!("Invalid environment Headers JSON: {error}"))?;
        let bearer_token_env_var = self.bearer_token_env_var.text();
        let mut config = McpServerConfig {
            name: self.name.text(),
            url: self.url.text(),
            bearer_token_env_var: (!bearer_token_env_var.trim().is_empty())
                .then(|| bearer_token_env_var.trim().to_owned()),
            http_headers,
            env_http_headers,
            enabled: self.enabled,
            startup_timeout_sec,
        };
        config.normalize();
        config.validate().map_err(|error| format!("{error:#}"))?;
        Ok(config)
    }
}

fn parse_json_or_default<T: serde::de::DeserializeOwned>(
    raw: &str,
    default: T,
) -> Result<T, serde_json::Error> {
    if raw.trim().is_empty() {
        Ok(default)
    } else {
        serde_json::from_str(raw)
    }
}

/// 设置弹窗状态：改动落在草稿上，Ctrl+S 才写回并持久化。
#[derive(Debug)]
pub struct SettingsUi {
    pub category: SettingsCategory,
    pub pane: SettingsPane,
    pub section: ModelsSection,
    /// 正在查看的供应商下标（未必是当前用于对话的供应商）。
    pub provider_pos: usize,
    pub model_pos: usize,
    /// 「外观」分类的选中字段。
    pub appearance_pos: usize,
    /// 「网络」分类的选中字段与代理 URL 编辑状态。
    pub network_pos: usize,
    /// 「键盘」分类的选中命令与组合键编辑状态。
    pub keybinding_pos: usize,
    pub keybinding_edit: Option<LineEdit>,
    /// 「上下文」分类的选中字段。
    pub context_pos: usize,
    /// 「MCP」分类的 Server 选中项、运行时状态和新增/编辑向导。
    pub mcp_pos: usize,
    pub mcp_statuses: BTreeMap<String, McpServerSnapshot>,
    pub mcp_wizard: Option<McpServerWizard>,
    pub proxy_edit: LineEdit,
    pub proxy_editing: bool,
    pub validation_error: Option<String>,
    pub draft: Settings,
    pub wizard: Option<ProviderWizard>,
    /// 「模型」分类的手动添加行。
    pub manual: LineEdit,
    pub manual_active: bool,
    /// 主界面 `s` 同步任务代号与错误信息。
    pub sync_id: Option<u64>,
    /// `sync_id` 对应的供应商下标，不能随当前浏览项变化。
    pub sync_provider: Option<usize>,
    pub sync_error: Option<String>,
    pub sync_error_provider: Option<usize>,
}

impl SettingsUi {
    fn new(settings: Settings) -> Self {
        let proxy_url = settings.proxy.url.clone().unwrap_or_default();
        Self {
            category: SettingsCategory::Models,
            pane: SettingsPane::Category,
            section: ModelsSection::Providers,
            provider_pos: settings.current_provider.min(settings.providers.len() - 1),
            model_pos: 0,
            appearance_pos: 0,
            network_pos: 0,
            keybinding_pos: 0,
            keybinding_edit: None,
            context_pos: 0,
            mcp_pos: 0,
            mcp_statuses: BTreeMap::new(),
            mcp_wizard: None,
            proxy_edit: LineEdit::from_text(&proxy_url),
            proxy_editing: false,
            validation_error: None,
            draft: settings,
            wizard: None,
            manual: LineEdit::default(),
            manual_active: false,
            sync_id: None,
            sync_provider: None,
            sync_error: None,
            sync_error_provider: None,
        }
    }

    /// 正在查看的供应商。
    fn viewed_provider(&self) -> &Provider {
        &self.draft.providers[self.provider_pos.min(self.draft.providers.len() - 1)]
    }

    fn apply_proxy_edit(&mut self) {
        let url = self.proxy_edit.text();
        self.draft.proxy.url = (!url.trim().is_empty()).then(|| url.trim().to_owned());
        self.draft.proxy.normalize();
    }

    fn reset_proxy_edit(&mut self) {
        self.proxy_edit = LineEdit::from_text(self.draft.proxy.url.as_deref().unwrap_or_default());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    SessionDefault,
    NextTurn,
}

/// 模型快速切换弹窗（Alt+M 会话默认 / Ctrl+M 下一轮临时覆盖）。
#[derive(Debug)]
pub struct ModelPicker {
    pub mode: PickerMode,
    pub active: ModelSelection,
    /// 当前选中模型所属的供应商下标（Tab 可按供应商快速跳转）。
    pub provider_idx: usize,
    pub selected: usize,
    /// 是否正在输入新模型名。
    pub editing: bool,
    pub buffer: Vec<char>,
    pub cursor: usize,
}

impl ModelPicker {
    fn new(settings: &Settings, active: ModelSelection, mode: PickerMode) -> Self {
        let provider_idx = settings
            .providers
            .iter()
            .position(|provider| provider.id == active.provider_id)
            .unwrap_or(settings.current_provider.min(settings.providers.len() - 1));
        let models = &settings.providers[provider_idx].models;
        let selected = models
            .iter()
            .position(|model| *model == active.model)
            .unwrap_or(0);
        Self {
            mode,
            active,
            provider_idx,
            selected,
            editing: false,
            buffer: Vec::new(),
            cursor: 0,
        }
    }

    fn switch_provider(&mut self, settings: &Settings, forward: bool) {
        let count = settings.providers.len();
        self.provider_idx = if forward {
            (self.provider_idx + 1) % count
        } else {
            (self.provider_idx + count - 1) % count
        };
        let provider = &settings.providers[self.provider_idx];
        self.selected = provider
            .models
            .iter()
            .position(|model| provider.id == self.active.provider_id && *model == self.active.model)
            .unwrap_or(0);
    }

    /// 在按 Provider 排列的统一模型列表中移动；到达首尾后保持不动。
    fn move_model(&mut self, settings: &Settings, forward: bool) {
        let Some(provider) = settings.providers.get(self.provider_idx) else {
            return;
        };
        if forward {
            if self.selected + 1 < provider.models.len() {
                self.selected += 1;
                return;
            }
            if let Some((provider_idx, _)) = settings
                .providers
                .iter()
                .enumerate()
                .skip(self.provider_idx + 1)
                .find(|(_, provider)| !provider.models.is_empty())
            {
                self.provider_idx = provider_idx;
                self.selected = 0;
            }
            return;
        }

        if self.selected > 0 && self.selected < provider.models.len() {
            self.selected -= 1;
            return;
        }
        if let Some((provider_idx, provider)) = settings.providers[..self.provider_idx]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, provider)| !provider.models.is_empty())
        {
            self.provider_idx = provider_idx;
            self.selected = provider.models.len() - 1;
        }
    }
}

/// 一次流式生成的运行状态，用于 footer 统计与中止后的事件过滤。
#[derive(Debug)]
pub struct StreamState {
    pub id: u64,
    /// 发起本次生成的会话；切换界面后仍按该 id 路由增量。
    pub session_id: String,
    pub started: Instant,
    pub chars: usize,
    pub done: bool,
    pub aborted: bool,
    pub usage: Option<Usage>,
    pub stop_reason: Option<StopReason>,
    pub metrics: Option<RequestMetrics>,
    finished_at: Option<Instant>,
    provider: Provider,
    proxy: ProxySettings,
    model: String,
    selection: ModelSelection,
    language: Lang,
    cancellation: CancellationToken,
    dirty: bool,
    last_checkpoint: Instant,
}

impl StreamState {
    pub fn generating(&self) -> bool {
        !self.done
    }

    /// 估算输出 token 数（字符数 / 4；无 usage 时的近似值）。
    fn estimated_tokens(&self) -> f64 {
        self.chars as f64 / 4.0
    }

    /// 生成速度（tok/s）；刚开始或没有内容时返回 `None`。
    pub fn tokens_per_sec(&self) -> Option<f64> {
        if self.chars == 0 {
            return None;
        }
        let elapsed = self
            .finished_at
            .unwrap_or_else(Instant::now)
            .duration_since(self.started)
            .as_secs_f64();
        if elapsed < 0.2 {
            return None;
        }
        let tokens = self.usage.map_or_else(
            || self.estimated_tokens(),
            |usage| usage.completion_tokens as f64,
        );
        Some(tokens / elapsed)
    }
}

/// Application.
#[derive(Debug)]
pub struct App {
    /// Is the application running?
    pub running: bool,
    /// Event handler.
    pub events: EventHandler,
    /// 已生效的设置。
    pub settings: Settings,
    /// 打开中的设置弹窗。
    pub modal: Option<SettingsUi>,
    /// 打开中的模型切换弹窗。
    pub picker: Option<ModelPicker>,
    /// 全部会话，按 `updated_at` 倒序。
    pub sessions: Vec<Session>,
    /// 当前打开的会话下标。
    pub current: usize,
    /// 键盘焦点。
    pub focus: Focus,
    /// 侧栏选择下标；`sessions.len()` 表示 Settings 入口。
    pub sidebar_pos: usize,
    /// Unicode-aware 多行聊天编辑器。
    pub editor: ChatEditor,
    /// 输入框星空的显示状态和下一帧截止时间。
    pub(crate) sparkle: Sparkle,
    pub(crate) chat_view: RefCell<ChatViewState>,
    pub(crate) activity_overlay: Option<ActivityOverlay>,
    pub(crate) mouse: RefCell<MouseMap>,
    /// 当前流式生成状态。
    pub stream: Option<StreamState>,
    /// 只作用于下一次成功发起请求的模型覆盖。
    pub next_turn_model: Option<ModelSelection>,
    stream_task: Option<JoinHandle<()>>,
    next_stream_id: u64,
    /// 模型同步任务代号计数器（向导与主界面共用）。
    sync_seq: u64,
    /// 聊天区底部的临时提示（错误、中止等）。
    notices: HashMap<String, Notice>,
    /// 当前错误详情弹窗；按 session id 固定来源，避免切换会话后串台。
    pub error_detail: Option<ErrorDetailState>,
    /// 当前会话 fenced code block 查看器。
    pub code_overlay: Option<CodeBlockOverlayState>,
    /// 会话重命名、搜索、恢复与压缩预览弹窗。
    pub conversation_overlay: Option<ConversationOverlay>,
    mcp_registry: McpRegistry,
    /// tick 计数，驱动 spinner 动画。
    pub ticks: u64,
}

impl App {
    /// Constructs a new instance of [`App`].
    pub fn new(mut settings: Settings, mut sessions: Vec<Session>) -> Self {
        settings.normalize();
        let mut notices = HashMap::new();
        if sessions.is_empty() {
            let session = Session::new();
            if let Err(err) = session.save() {
                tracing::error!(
                    session = ?session.id,
                    error = %format!("{err:#}"),
                    "初始会话保存失败"
                );
                let message = settings
                    .language
                    .notice_session_storage_failed(&err.to_string());
                notices.insert(session.id.clone(), Notice::warning(message));
            }
            sessions.push(session);
        }
        let events = EventHandler::new();
        let mcp_registry = McpRegistry::new(events.sender());
        let runtime_mcp_servers = settings.runtime_mcp_servers();
        mcp_registry.reconfigure(&runtime_mcp_servers, &settings.proxy);
        let mut app = Self {
            running: true,
            events,
            sparkle: Sparkle::new(settings.whimsy),
            settings,
            modal: None,
            picker: None,
            current: 0,
            sessions,
            focus: Focus::default(),
            sidebar_pos: 0,
            editor: ChatEditor::default(),
            chat_view: RefCell::default(),
            activity_overlay: None,
            mouse: RefCell::default(),
            stream: None,
            next_turn_model: None,
            stream_task: None,
            next_stream_id: 0,
            sync_seq: 0,
            notices,
            error_detail: None,
            code_overlay: None,
            conversation_overlay: None,
            mcp_registry,
            ticks: 0,
        };
        app.open_next_recovery();
        app
    }

    /// 当前打开的会话。
    pub fn open_session(&self) -> &Session {
        &self.sessions[self.current]
    }

    /// 下一轮覆盖 → 会话默认 → 应用默认。
    pub fn effective_selection(&self) -> ModelSelection {
        self.next_turn_model
            .as_ref()
            .filter(|selection| self.settings.selection_available(selection))
            .or_else(|| {
                self.open_session()
                    .default_model
                    .as_ref()
                    .filter(|selection| self.settings.selection_available(selection))
            })
            .cloned()
            .unwrap_or_else(|| self.settings.default_selection())
    }

    pub fn provider_for_selection(&self, selection: &ModelSelection) -> &Provider {
        self.settings
            .provider_by_id(&selection.provider_id)
            .unwrap_or_else(|| self.settings.provider())
    }

    pub fn displayed_selection(&self) -> ModelSelection {
        self.stream
            .as_ref()
            .filter(|state| state.generating() && state.session_id == self.open_session().id)
            .map(|state| state.selection.clone())
            .unwrap_or_else(|| self.effective_selection())
    }

    pub fn has_next_turn_model(&self) -> bool {
        self.next_turn_model
            .as_ref()
            .is_some_and(|selection| self.settings.selection_available(selection))
    }

    fn open_activity_details(&mut self) {
        let preferred = self.chat_view.borrow().visible_activity.clone();
        self.activity_overlay = Some(ActivityOverlay::new(self.open_session(), preferred));
    }

    /// 是否正在生成。
    pub fn generating(&self) -> bool {
        self.stream.as_ref().is_some_and(StreamState::generating)
    }

    /// 指定会话是否是当前流式生成的目标。
    pub fn session_generating(&self, session_id: &str) -> bool {
        self.stream
            .as_ref()
            .is_some_and(|state| state.generating() && state.session_id == session_id)
    }

    /// 当前打开会话的临时提示。
    pub fn notice(&self) -> Option<&Notice> {
        self.notices.get(&self.open_session().id)
    }

    pub fn detailed_error(&self) -> Option<&RuntimeError> {
        let detail = self.error_detail.as_ref()?;
        self.notices
            .get(&detail.session_id)
            .and_then(|notice| notice.error.as_ref())
    }

    pub fn context_budget(&self) -> ContextBudget {
        let selection = self.effective_selection();
        let provider = self.provider_for_selection(&selection);
        let model = provider.settings_for_model(&selection.model);
        build_budget(
            &model,
            &self.settings.context,
            ContextInputs {
                system_instructions: &[],
                summary: self.open_session().summary.as_ref(),
                messages: &self.open_session().messages,
            },
        )
    }

    /// 当前活动上下文的保守输入 token 估算。
    #[cfg(test)]
    pub fn context_tokens(&self) -> usize {
        self.context_budget().input_tokens
    }

    /// Run the application's main loop.
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        let result = self.run_loop(&mut terminal).await;
        if self.generating() {
            let _ = self.abort_stream(CancellationReason::ApplicationExit);
        }
        self.mcp_registry.shutdown().await;
        result
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> color_eyre::Result<()> {
        self.events.watch_whimsy(Settings::config_path()?);
        while self.running {
            self.sparkle
                .set_enabled(self.settings.whimsy, Instant::now());
            // 动画重绘前隐藏光标，画完后由 Ratatui 恢复位置，避免星点刷新时闪移。
            terminal.hide_cursor()?;
            terminal.draw(|frame| ui::draw(frame, self))?;
            self.events
                .set_animation_enabled(self.periodic_tick_required());
            let next_frame = self.sparkle.next_frame();
            let event = tokio::select! {
                event = self.events.next() => event?,
                _ = async {
                    if let Some(deadline) = next_frame {
                        tokio::time::sleep_until(deadline.into()).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => continue,
            };
            match event {
                Event::Tick => self.tick(),
                Event::Crossterm(event) => match event {
                    crossterm::event::Event::Key(key_event)
                        if key_event.kind == crossterm::event::KeyEventKind::Press =>
                    {
                        self.handle_key_events(key_event)?
                    }
                    // 括号粘贴：整块进入当前聚焦的输入缓冲
                    crossterm::event::Event::Paste(text) => self.handle_paste(text),
                    crossterm::event::Event::Mouse(mouse) => {
                        self.handle_mouse_events(mouse);
                    }
                    crossterm::event::Event::FocusGained => self.sparkle.set_terminal_focus(true),
                    crossterm::event::Event::FocusLost => {
                        self.sparkle.set_terminal_focus(false);
                        self.mouse.borrow_mut().dragging_input = false;
                    }
                    _ => {}
                },
                Event::App(app_event) => self.handle_app_event(app_event),
                Event::Error(message) => return Err(color_eyre::eyre::eyre!(message)),
            }
        }
        Ok(())
    }

    fn periodic_tick_required(&self) -> bool {
        if self.generating() {
            return true;
        }
        let Some(modal) = &self.modal else {
            return false;
        };
        modal.sync_id.is_some()
            || modal.wizard.as_ref().is_some_and(|wizard| wizard.fetching)
            || modal.mcp_statuses.values().any(|snapshot| {
                matches!(
                    snapshot.status,
                    McpServerStatus::Starting | McpServerStatus::Stopping
                )
            })
    }

    /// 把粘贴的文本送入当前聚焦的输入框（去掉首尾空白与换行）。
    fn handle_paste(&mut self, text: String) {
        let normalized = normalize_newlines(&text);
        let trimmed = normalized.trim_matches('\n');
        if trimmed.is_empty() {
            return;
        }
        if self.error_detail.is_some()
            || self.activity_overlay.is_some()
            || self.code_overlay.is_some()
        {
            return;
        }
        if let Some(overlay) = &mut self.conversation_overlay {
            match overlay {
                ConversationOverlay::Rename { edit, .. }
                | ConversationOverlay::Search { edit, .. } => edit.insert_text(trimmed),
                ConversationOverlay::Recovery(_) | ConversationOverlay::Compact { .. } => {}
            }
            return;
        }
        // 目标缓冲：模型切换弹窗的新增行 > 向导输入行/手动行 > 设置手动行 > 聊天输入框
        if let Some(picker) = &mut self.picker {
            if picker.editing {
                insert_text(&mut picker.buffer, &mut picker.cursor, trimmed, false);
            }
            return;
        }
        if let Some(modal) = &mut self.modal {
            if let Some(edit) = &mut modal.keybinding_edit {
                edit.insert_text(trimmed);
                modal.validation_error = None;
                return;
            }
            if let Some(wizard) = &mut modal.mcp_wizard {
                let field = wizard.field;
                wizard.line_mut(field).insert_text(trimmed);
                wizard.error = None;
                return;
            }
            if let Some(wizard) = &mut modal.wizard {
                // 向导第 2 步粘贴到当前字段；第 3 步粘贴到手动添加行
                if wizard.step == 2 {
                    let edit = wizard.line_mut(wizard.field);
                    edit.insert_text(trimmed);
                    return;
                }
                if wizard.manual_active {
                    wizard.manual.insert_text(trimmed);
                    return;
                }
                return;
            }
            if modal.manual_active {
                modal.manual.insert_text(trimmed);
                return;
            }
            return;
        }
        // 聊天输入框：粘贴不自动发送
        self.editor.insert_text(trimmed);
    }

    /// Handles the key events and updates the state of [`App`].
    pub fn handle_key_events(&mut self, key_event: KeyEvent) -> color_eyre::Result<()> {
        let ctrl_c = matches!(key_event.code, KeyCode::Char('c' | 'C'))
            && key_event.modifiers.contains(KeyModifiers::CONTROL);

        // 活动任务优先级最高：即使设置/模型/错误弹窗打开，Esc 与 Ctrl+C 都先取消生成。
        if self.generating() && (key_event.code == KeyCode::Esc || ctrl_c) {
            if let Some(session_id) = self.stream.as_ref().map(|state| state.session_id.clone()) {
                self.set_notice(
                    session_id,
                    self.settings.language.texts().notice_aborted.to_owned(),
                );
            }
            let _ = self.abort_stream(CancellationReason::User);
            return Ok(());
        }

        // 无活动任务时 Ctrl+C 在任何焦点层级都退出，避免弹窗静默吞键。
        if ctrl_c {
            self.events.send(AppEvent::Quit);
            return Ok(());
        }

        // 错误详情是最上层弹窗，Esc/F2 返回；PageUp/PageDown 可滚动诊断链。
        if self.error_detail.is_some() {
            if matches!(key_event.code, KeyCode::Esc | KeyCode::F(2)) {
                self.error_detail = None;
            } else if let Some(detail) = &mut self.error_detail {
                match key_event.code {
                    KeyCode::PageUp | KeyCode::Up => {
                        detail.scroll = detail.scroll.saturating_sub(4);
                    }
                    KeyCode::PageDown | KeyCode::Down => {
                        detail.scroll = detail.scroll.saturating_add(4);
                    }
                    _ => {}
                }
            }
            return Ok(());
        }

        if self.activity_overlay.is_some() {
            ui::activity::handle_key(self, key_event);
            return Ok(());
        }

        if self.code_overlay.is_some() {
            self.handle_code_overlay_key(key_event);
            return Ok(());
        }

        if self.conversation_overlay.is_some() {
            self.handle_conversation_overlay_key(key_event);
            return Ok(());
        }
        // 设置弹窗打开时独占按键
        if self.modal.is_some() {
            self.handle_settings_key(key_event);
            return Ok(());
        }
        // 模型切换弹窗打开时独占按键
        if self.picker.is_some() {
            self.handle_picker_key(key_event);
            return Ok(());
        }
        if key_event.code == KeyCode::F(3) {
            self.open_activity_details();
            return Ok(());
        }
        if key_event.code == KeyCode::F(2)
            && self.notice().is_some_and(|notice| notice.error.is_some())
        {
            self.error_detail = Some(ErrorDetailState {
                session_id: self.open_session().id.clone(),
                scroll: 0,
            });
            return Ok(());
        }
        if key_event.code == KeyCode::F(4) {
            self.open_code_viewer();
            return Ok(());
        }
        if matches!(key_event.code, KeyCode::Char('f' | 'F'))
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.open_session_search();
            return Ok(());
        }
        // Alt+M 设置会话默认；Ctrl+M 只覆盖下一轮。
        if matches!(key_event.code, KeyCode::Char('m' | 'M'))
            && key_event
                .modifiers
                .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT))
        {
            let mode = if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                PickerMode::NextTurn
            } else {
                PickerMode::SessionDefault
            };
            let active = match mode {
                PickerMode::SessionDefault => self
                    .open_session()
                    .default_model
                    .as_ref()
                    .filter(|selection| self.settings.selection_available(selection))
                    .cloned()
                    .unwrap_or_else(|| self.settings.default_selection()),
                PickerMode::NextTurn => self.effective_selection(),
            };
            self.picker = Some(ModelPicker::new(&self.settings, active, mode));
            return Ok(());
        }
        match self.focus {
            Focus::Input => self.handle_input_key(key_event),
            Focus::Sidebar => self.handle_sidebar_key(key_event),
        }
        Ok(())
    }

    /// Handles the tick event of the terminal.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        self.checkpoint_stream_if_due();
    }

    pub fn search_results(&self, query: &str) -> Vec<usize> {
        self.sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| session.matches_query(query).then_some(index))
            .collect()
    }

    fn open_session_search(&mut self) {
        self.conversation_overlay = Some(ConversationOverlay::Search {
            edit: LineEdit::default(),
            selected: 0,
        });
    }

    fn open_rename(&mut self, session_id: String) {
        let title = self
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.display_title(self.settings.language.texts().new_session_title))
            .unwrap_or_default();
        self.conversation_overlay = Some(ConversationOverlay::Rename {
            session_id,
            edit: LineEdit::from_text(&title),
            error: None,
        });
    }

    fn open_compaction_preview(&mut self) {
        let selection = self.effective_selection();
        let model = self
            .provider_for_selection(&selection)
            .settings_for_model(&selection.model);
        let session_id = self.open_session().id.clone();
        let Some(plan) =
            prepare_compaction(self.open_session(), &model, &self.settings.context, false)
        else {
            self.set_current_notice(
                self.settings
                    .language
                    .notice_nothing_to_compact()
                    .to_owned(),
            );
            return;
        };
        self.conversation_overlay = Some(ConversationOverlay::Compact {
            session_id,
            plan,
            scroll: 0,
        });
    }

    fn handle_conversation_overlay_key(&mut self, key_event: KeyEvent) {
        let Some(overlay) = self.conversation_overlay.take() else {
            return;
        };
        match overlay {
            ConversationOverlay::Rename {
                session_id,
                mut edit,
                error: _,
            } => match key_event.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let title = edit.text().trim().to_owned();
                    if title.is_empty() {
                        self.conversation_overlay = Some(ConversationOverlay::Rename {
                            session_id,
                            edit,
                            error: Some(self.settings.language.rename_empty_error().to_owned()),
                        });
                        return;
                    }
                    let result = self.mutate_session_and_save(&session_id, |session| {
                        session.title = Some(title);
                        session.updated_at = Utc::now();
                    });
                    if let Err(err) = result {
                        self.conversation_overlay = Some(ConversationOverlay::Rename {
                            session_id,
                            edit,
                            error: Some(err),
                        });
                    }
                }
                _ => {
                    edit.accept_key(key_event);
                    self.conversation_overlay = Some(ConversationOverlay::Rename {
                        session_id,
                        edit,
                        error: None,
                    });
                }
            },
            ConversationOverlay::Search {
                mut edit,
                mut selected,
            } => {
                let query = edit.text();
                let result_count = self.search_results(&query).len();
                match key_event.code {
                    KeyCode::Esc => {}
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        selected = (selected + 1).min(result_count.saturating_sub(1));
                    }
                    KeyCode::Enter => {
                        if let Some(index) = self.search_results(&query).get(selected).copied() {
                            let id = self.sessions[index].id.clone();
                            self.sidebar_pos = index;
                            self.switch_session(&id);
                        }
                        return;
                    }
                    _ => {
                        if edit.accept_key(key_event) {
                            selected = 0;
                        }
                    }
                }
                if key_event.code != KeyCode::Esc {
                    let count = self.search_results(&edit.text()).len();
                    selected = selected.min(count.saturating_sub(1));
                    self.conversation_overlay =
                        Some(ConversationOverlay::Search { edit, selected });
                }
            }
            ConversationOverlay::Recovery(mut state) => match key_event.code {
                KeyCode::Left | KeyCode::Up => {
                    state.selected = state.selected.saturating_sub(1);
                    state.error = None;
                    self.conversation_overlay = Some(ConversationOverlay::Recovery(state));
                }
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                    state.selected = (state.selected + 1).min(RecoveryChoice::ALL.len() - 1);
                    state.error = None;
                    self.conversation_overlay = Some(ConversationOverlay::Recovery(state));
                }
                KeyCode::Esc => {
                    if !self.resolve_recovery(&mut state, RecoveryChoice::Keep) {
                        self.conversation_overlay = Some(ConversationOverlay::Recovery(state));
                    } else {
                        self.open_next_recovery();
                    }
                }
                KeyCode::Enter => {
                    let choice = RecoveryChoice::ALL[state.selected];
                    if !self.resolve_recovery(&mut state, choice) {
                        self.conversation_overlay = Some(ConversationOverlay::Recovery(state));
                    } else {
                        self.open_next_recovery();
                    }
                }
                _ => self.conversation_overlay = Some(ConversationOverlay::Recovery(state)),
            },
            ConversationOverlay::Compact {
                session_id,
                plan,
                mut scroll,
            } => match key_event.code {
                KeyCode::Esc => {}
                KeyCode::Up | KeyCode::PageUp => {
                    scroll = scroll.saturating_sub(4);
                    self.conversation_overlay = Some(ConversationOverlay::Compact {
                        session_id,
                        plan,
                        scroll,
                    });
                }
                KeyCode::Down | KeyCode::PageDown => {
                    scroll = scroll.saturating_add(4);
                    self.conversation_overlay = Some(ConversationOverlay::Compact {
                        session_id,
                        plan,
                        scroll,
                    });
                }
                KeyCode::Enter => {
                    let archived = plan.archive_count;
                    let result = self.mutate_session_and_save(&session_id, move |session| {
                        plan.apply(session);
                    });
                    match result {
                        Ok(()) => self.set_notice(
                            session_id,
                            self.settings.language.notice_compacted(archived, false),
                        ),
                        Err(err) => self.set_notice(
                            session_id,
                            self.settings.language.notice_session_storage_failed(&err),
                        ),
                    }
                }
                _ => {
                    self.conversation_overlay = Some(ConversationOverlay::Compact {
                        session_id,
                        plan,
                        scroll,
                    });
                }
            },
        }
    }

    fn open_code_viewer(&mut self) {
        let blocks = self
            .open_session()
            .messages
            .iter()
            .flat_map(|message| ui::markdown::extract_code_blocks(&message.content()))
            .collect::<Vec<_>>();
        if blocks.is_empty() {
            let message = match self.settings.language {
                Lang::Zh => "当前会话没有 fenced code block",
                Lang::En => "This conversation has no fenced code block",
            };
            self.set_current_info(message.to_owned());
            return;
        }
        self.code_overlay = Some(CodeBlockOverlayState {
            selected: blocks.len() - 1,
            blocks,
            vertical_scroll: 0,
            horizontal_scroll: 0,
            status: None,
        });
    }

    fn handle_code_overlay_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let copy_binding = self.settings.keybindings.copy;
        let mut close = false;
        let mut copy = false;
        if self
            .code_overlay
            .as_ref()
            .is_some_and(|overlay| overlay.blocks.is_empty())
        {
            self.code_overlay = None;
            return;
        }
        if let Some(overlay) = &mut self.code_overlay {
            overlay.selected = overlay.selected.min(overlay.blocks.len() - 1);
            match key_event.code {
                KeyCode::Esc | KeyCode::F(4) => close = true,
                KeyCode::Char('[') if plain => {
                    overlay.selected = overlay.selected.saturating_sub(1);
                    overlay.vertical_scroll = 0;
                    overlay.horizontal_scroll = 0;
                    overlay.status = None;
                }
                KeyCode::Char(']') if plain => {
                    overlay.selected = (overlay.selected + 1).min(overlay.blocks.len() - 1);
                    overlay.vertical_scroll = 0;
                    overlay.horizontal_scroll = 0;
                    overlay.status = None;
                }
                KeyCode::Up => {
                    overlay.vertical_scroll = overlay.vertical_scroll.saturating_sub(1);
                }
                KeyCode::Down => {
                    overlay.vertical_scroll = overlay.vertical_scroll.saturating_add(1);
                }
                KeyCode::PageUp => {
                    overlay.vertical_scroll = overlay.vertical_scroll.saturating_sub(12);
                }
                KeyCode::PageDown => {
                    overlay.vertical_scroll = overlay.vertical_scroll.saturating_add(12);
                }
                KeyCode::Left => {
                    overlay.horizontal_scroll = overlay.horizontal_scroll.saturating_sub(4);
                }
                KeyCode::Right => {
                    let max_scroll = overlay.blocks[overlay.selected]
                        .content
                        .lines()
                        .map(UnicodeWidthStr::width)
                        .max()
                        .unwrap_or(0)
                        .saturating_sub(1);
                    overlay.horizontal_scroll =
                        overlay.horizontal_scroll.saturating_add(4).min(max_scroll);
                }
                KeyCode::Home => overlay.horizontal_scroll = 0,
                KeyCode::Char('c' | 'C') if plain => copy = true,
                _ if copy_binding.matches(key_event) => copy = true,
                _ => {}
            }
        }
        if copy
            && let Some(overlay) = &mut self.code_overlay
            && let Some(block) = overlay.blocks.get(overlay.selected)
        {
            overlay.status = Some(match clipboard::copy(&block.content) {
                Ok(()) => match self.settings.language {
                    Lang::Zh => "已复制代码块".to_owned(),
                    Lang::En => "Code block copied".to_owned(),
                },
                Err(error) => match self.settings.language {
                    Lang::Zh => format!("复制失败：{error}"),
                    Lang::En => format!("Copy failed: {error}"),
                },
            });
        }
        if close {
            self.code_overlay = None;
        }
    }

    fn open_next_recovery(&mut self) {
        if self.generating() || self.conversation_overlay.is_some() {
            return;
        }
        let target = self
            .sessions
            .iter()
            .enumerate()
            .find_map(|(session_index, session)| {
                session
                    .first_streaming_message()
                    .map(|message_index| (session_index, message_index))
            });
        let Some((session_index, message_index)) = target else {
            return;
        };
        self.current = session_index;
        self.sidebar_pos = session_index;
        self.chat_view.borrow_mut().follow_latest();
        self.conversation_overlay = Some(ConversationOverlay::Recovery(RecoveryState {
            session_id: self.sessions[session_index].id.clone(),
            message_index,
            selected: 0,
            error: None,
        }));
    }

    fn resolve_recovery(&mut self, state: &mut RecoveryState, choice: RecoveryChoice) -> bool {
        let session_index = self
            .sessions
            .iter()
            .position(|session| session.id == state.session_id);
        let Some(session_index) = session_index else {
            return true;
        };
        if !self.sessions[session_index]
            .messages
            .get(state.message_index)
            .is_some_and(|message| message.status == MessageStatus::Streaming)
        {
            return true;
        }

        match choice {
            RecoveryChoice::Keep => {
                let result = self.mutate_session_and_save(&state.session_id, |session| {
                    let _ =
                        session.resolve_incomplete(state.message_index, IncompleteRecovery::Keep);
                });
                if let Err(err) = result {
                    state.error = Some(err);
                    return false;
                }
            }
            RecoveryChoice::Discard => {
                let result = self.mutate_session_and_save(&state.session_id, |session| {
                    let _ = session
                        .resolve_incomplete(state.message_index, IncompleteRecovery::Discard);
                });
                if let Err(err) = result {
                    state.error = Some(err);
                    return false;
                }
            }
            RecoveryChoice::Continue => {
                let message = self.sessions[session_index].messages[state.message_index].clone();
                let Some(selection) = message.model.clone() else {
                    state.error = Some(self.settings.language.recovery_missing_model().to_owned());
                    return false;
                };
                if !self.settings.selection_available(&selection) {
                    state.error = Some(
                        self.settings
                            .language
                            .recovery_model_unavailable(&selection.provider_name, &selection.model),
                    );
                    return false;
                }
                let provider = self.provider_for_selection(&selection).clone();
                let mut model_settings = provider.settings_for_model(&selection.model);
                if let Some(parameters) = message.parameters.clone() {
                    model_settings.parameters = parameters;
                }
                let result = self.mutate_session_and_save(&state.session_id, |session| {
                    let _ = session
                        .resolve_incomplete(state.message_index, IncompleteRecovery::Continue);
                });
                if let Err(err) = result {
                    state.error = Some(err);
                    return false;
                }
                self.current = session_index;
                self.sidebar_pos = session_index;
                let prompt = self.settings.language.recovery_continue_prompt().to_owned();
                if let Err(error) =
                    self.start_turn(prompt, selection, provider, model_settings, true)
                {
                    let summary = self.settings.language.runtime_error_notice(&error);
                    self.set_error_notice(state.session_id.clone(), summary, error);
                }
            }
        }
        true
    }

    fn mutate_session_and_save(
        &mut self,
        session_id: &str,
        update: impl FnOnce(&mut Session),
    ) -> Result<(), String> {
        let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Err("session no longer exists".to_owned());
        };
        let original = self.sessions[index].clone();
        update(&mut self.sessions[index]);
        if let Err(err) = self.sessions[index].save() {
            self.sessions[index] = original;
            return Err(err.to_string());
        }
        Ok(())
    }

    fn checkpoint_stream_if_due(&mut self) {
        let Some(session_id) = self.stream.as_ref().and_then(|state| {
            (state.generating()
                && state.dirty
                && state.last_checkpoint.elapsed() >= std::time::Duration::from_secs(1))
            .then(|| state.session_id.clone())
        }) else {
            return;
        };
        let save_error = self
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .and_then(|session| session.save().err());
        if let Some(error) = save_error {
            self.report_session_storage_error(&session_id, &error);
            return;
        }
        if let Some(state) = self
            .stream
            .as_mut()
            .filter(|state| state.session_id == session_id)
        {
            state.dirty = false;
            state.last_checkpoint = Instant::now();
        }
    }

    /// 处理聊天事件（流式生成、标题生成与模型同步任务）。
    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Quit => self.quit(),
            AppEvent::WhimsyChanged(enabled) => {
                if let Some(modal) = self.modal.as_mut()
                    && modal.draft.whimsy == self.settings.whimsy
                {
                    modal.draft.whimsy = enabled;
                }
                self.settings.whimsy = enabled;
                self.sparkle.set_enabled(enabled, Instant::now());
            }
            AppEvent::ModelStream { stream, event } => self.on_model_stream(stream, event),
            AppEvent::McpRuntime(event) => self.on_mcp_runtime(event),
            AppEvent::TitleGenerated { session, title } => {
                // 生成期间可能已切换会话，按 id 定位再写入
                let save_error = if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|candidate| candidate.id == session)
                    && session.title.is_none()
                {
                    session.title = Some(title);
                    session.save().err()
                } else {
                    None
                };
                if let Some(err) = save_error {
                    self.report_session_storage_error(&session, &err);
                }
            }
            AppEvent::ModelsSynced { task, models } => self.on_models_synced(task, models),
        }
    }

    fn on_mcp_runtime(&mut self, event: McpRuntimeEvent) {
        let McpRuntimeEvent::Changed {
            server_name,
            generation,
        } = event;
        let Some(snapshot) = self.mcp_registry.snapshot(&server_name) else {
            return;
        };
        if snapshot.generation != generation {
            return;
        }

        if let Some(modal) = self.modal.as_mut() {
            modal.mcp_statuses.insert(server_name, snapshot);
        }
    }

    fn on_model_stream(&mut self, stream: u64, event: ModelStreamEvent) {
        match event {
            ModelStreamEvent::AssistantTextDelta { text } => self.on_delta(stream, text),
            ModelStreamEvent::ReasoningDelta { text } => self.on_reasoning(stream, text),
            ModelStreamEvent::System { message } => self.on_system_event(stream, message),
            event @ (ModelStreamEvent::ToolCallDelta { .. }
            | ModelStreamEvent::ToolCallReady { .. }
            | ModelStreamEvent::ToolCall { .. }
            | ModelStreamEvent::ToolResult { .. }) => self.on_tool_event(stream, event),
            ModelStreamEvent::Completed {
                usage,
                stop_reason,
                metrics,
            } => self.on_done(stream, usage, stop_reason, metrics),
            ModelStreamEvent::Error { error, metrics } => self.on_error(stream, error, metrics),
            ModelStreamEvent::Cancelled { reason, metrics } => {
                self.on_cancelled(stream, reason, metrics)
            }
        }
    }

    fn active_stream_target(&self, stream: u64) -> Option<(String, ModelSelection)> {
        self.stream
            .as_ref()
            .filter(|state| state.id == stream && state.generating())
            .map(|state| (state.session_id.clone(), state.selection.clone()))
    }

    /// 助手正文增量：追加到发起会话的 streaming 消息。
    fn on_delta(&mut self, stream: u64, text: String) {
        let (session_id, selection) = {
            let Some(state) = &mut self.stream else {
                return;
            };
            if state.id != stream || !state.generating() {
                return;
            }
            state.chars = state.chars.saturating_add(text.chars().count());
            state.dirty = true;
            (state.session_id.clone(), state.selection.clone())
        };
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return;
        };
        stream_message_mut(session, selection).append_text(&text);
    }

    fn on_reasoning(&mut self, stream: u64, text: String) {
        let Some((session_id, selection)) = self.active_stream_target(stream) else {
            return;
        };
        let updated = if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            stream_message_mut(session, selection).append_reasoning(&text);
            true
        } else {
            false
        };
        if updated {
            self.mark_stream_dirty(stream);
        }
    }

    fn on_system_event(&mut self, stream: u64, message: String) {
        let Some((session_id, selection)) = self.active_stream_target(stream) else {
            return;
        };
        let updated = if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            stream_message_mut(session, selection).push_system_event(message);
            true
        } else {
            false
        };
        if updated {
            self.mark_stream_dirty(stream);
        }
    }

    fn on_tool_event(&mut self, stream: u64, event: ModelStreamEvent) {
        let Some((session_id, selection)) = self.active_stream_target(stream) else {
            return;
        };
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            let message = stream_message_mut(session, selection);
            match event {
                ModelStreamEvent::ToolCallDelta {
                    round,
                    index,
                    call_id,
                    name,
                    arguments,
                    replace,
                } => message.tool_delta(round, index, call_id, name, arguments, replace),
                ModelStreamEvent::ToolCallReady {
                    round,
                    index,
                    call_id,
                    server,
                    name,
                    arguments,
                } => message.tool_ready(round, index, call_id, server, name, arguments),
                ModelStreamEvent::ToolCall { round, index } => message.tool_started(round, index),
                ModelStreamEvent::ToolResult {
                    round,
                    index,
                    output,
                    is_error,
                } => message.tool_result(round, index, output, is_error),
                _ => unreachable!("non-tool event routed to tool reducer"),
            }
            self.mark_stream_dirty(stream);
        }
    }

    fn mark_stream_dirty(&mut self, stream: u64) {
        if let Some(state) = self
            .stream
            .as_mut()
            .filter(|state| state.id == stream && state.generating())
        {
            state.dirty = true;
        }
    }

    /// 标记流结束；流代号不匹配（已过期）时返回 `None`。
    fn finish_stream(&mut self, stream: u64) -> Option<GenerationContext> {
        let state = self.stream.as_mut()?;
        if state.id != stream || !state.generating() {
            return None;
        }
        state.done = true;
        state.finished_at = Some(Instant::now());
        let context = GenerationContext {
            session_id: state.session_id.clone(),
            provider: state.provider.clone(),
            proxy: state.proxy.clone(),
            model: state.model.clone(),
            language: state.language,
        };
        self.stream_task = None;
        Some(context)
    }

    /// 一次生成成功结束。
    fn on_done(
        &mut self,
        stream: u64,
        usage: Option<Usage>,
        stop_reason: StopReason,
        metrics: RequestMetrics,
    ) {
        let Some(context) = self.finish_stream(stream) else {
            return;
        };
        if let Some(state) = &mut self.stream {
            state.usage = usage;
            state.stop_reason = Some(stop_reason.clone());
            state.metrics = Some(metrics);
        }
        self.set_stream_message_status(&context.session_id, MessageStatus::Completed, None);
        if !matches!(stop_reason, StopReason::Stop | StopReason::StopSequence) {
            let notice = self.settings.language.stop_reason_notice(&stop_reason);
            self.set_notice(context.session_id.clone(), notice);
        }
        self.finalize_exchange(context);
        self.open_next_recovery();
    }

    fn on_error(&mut self, stream: u64, mut error: RuntimeError, metrics: RequestMetrics) {
        let Some(context) = self.finish_stream(stream) else {
            return;
        };
        if error.request_id.is_none() {
            error.request_id = metrics.request_id.clone();
        }
        if let Some(state) = &mut self.stream {
            state.metrics = Some(metrics);
        }
        self.set_stream_message_status(
            &context.session_id,
            MessageStatus::Failed,
            Some(error.snapshot()),
        );
        let summary = self.settings.language.runtime_error_notice(&error);
        self.set_error_notice(context.session_id.clone(), summary, error);
        self.finalize_exchange(context);
        self.open_next_recovery();
    }

    fn on_cancelled(&mut self, stream: u64, _reason: CancellationReason, metrics: RequestMetrics) {
        let Some(context) = self.finish_stream(stream) else {
            return;
        };
        if let Some(state) = &mut self.stream {
            state.aborted = true;
            state.metrics = Some(metrics);
        }
        self.set_stream_message_status(&context.session_id, MessageStatus::Cancelled, None);
        self.set_notice(
            context.session_id.clone(),
            self.settings.language.texts().notice_aborted.to_owned(),
        );
        self.finalize_exchange(context);
        self.open_next_recovery();
    }

    fn set_stream_message_status(
        &mut self,
        session_id: &str,
        status: MessageStatus,
        failure: Option<crate::runtime::error::RuntimeErrorSnapshot>,
    ) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            && let Some(message) = session.messages.iter_mut().rev().find(|message| {
                message.role == Role::Assistant && message.status == MessageStatus::Streaming
            })
        {
            message.finish(status);
            message.failure = failure;
        }
    }

    /// 模型同步结果路由：向导或设置主界面。
    fn on_models_synced(&mut self, task: u64, models: Result<ModelCatalog, String>) {
        let Some(modal) = self.modal.as_mut() else {
            return;
        };

        if let Some(wizard) = modal.wizard.as_mut()
            && wizard.id == task
        {
            wizard.fetching = false;
            match models {
                Ok(catalog) => {
                    let previously_selected: Vec<String> = wizard
                        .fetched
                        .as_ref()
                        .into_iter()
                        .flatten()
                        .zip(wizard.selected.iter())
                        .filter(|(_, selected)| **selected)
                        .map(|(model, _)| model.clone())
                        .collect();
                    let (list, settings) =
                        merge_model_catalog(std::mem::take(&mut wizard.fetched_settings), catalog);
                    wizard.selected = list
                        .iter()
                        .map(|model| previously_selected.contains(model))
                        .collect();
                    wizard.fetched = Some(list);
                    wizard.fetched_settings = settings;
                    wizard.list_pos = 0;
                    wizard.error = None;
                    wizard.warned = false;
                }
                Err(err) => wizard.error = Some(err),
            }
            return;
        }

        if modal.sync_id == Some(task) {
            modal.sync_id = None;
            let target = modal.sync_provider.take();
            match models {
                Ok(catalog) => {
                    modal.sync_error = None;
                    modal.sync_error_provider = None;
                    if let Some(index) = target.filter(|index| *index < modal.draft.providers.len())
                    {
                        let provider = &mut modal.draft.providers[index];
                        let (models, settings) = merge_model_catalog(
                            std::mem::take(&mut provider.model_settings),
                            catalog,
                        );
                        provider.models = models;
                        provider.model_settings = settings;
                        if index == modal.draft.current_provider
                            && !provider.models.is_empty()
                            && !provider.models.contains(&modal.draft.model)
                        {
                            modal.draft.model = provider.models[0].clone();
                        }
                        if target == Some(modal.provider_pos) {
                            modal.model_pos = 0;
                        }
                    }
                }
                Err(err) => {
                    modal.sync_error = Some(err);
                    modal.sync_error_provider = target;
                }
            }
        }
    }

    /// 保存当前会话、重排侧栏顺序，并在首轮交互后触发标题生成。
    fn finalize_exchange(&mut self, context: GenerationContext) {
        let open_id = self.open_session().id.clone();
        let sidebar_id = self
            .sessions
            .get(self.sidebar_pos)
            .map(|session| session.id.clone());
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == context.session_id)
        else {
            return;
        };
        session.updated_at = Utc::now();
        let save_error = session.save().err();
        let exchange = session.first_exchange();
        let needs_title = session.title.is_none();
        self.sessions
            .sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        self.current = self
            .sessions
            .iter()
            .position(|session| session.id == open_id)
            .unwrap_or(0);
        self.sidebar_pos = sidebar_id
            .and_then(|id| self.sessions.iter().position(|session| session.id == id))
            .unwrap_or(self.current);

        if let Some(err) = save_error {
            self.report_session_storage_error(&context.session_id, &err);
        }

        if let Some((user, reply)) = exchange
            && !user.is_empty()
            && !reply.is_empty()
            && needs_title
        {
            let sender = self.events.sender();
            llm::spawn_generate_title(
                llm::TitleGenerationRequest {
                    provider: context.provider,
                    proxy: context.proxy,
                    model: context.model,
                    lang: context.language,
                    session_id: context.session_id,
                    first_user: user,
                    first_reply: reply,
                },
                sender,
            );
        }
    }

    /// 发送输入框内容并启动流式生成。
    fn submit(&mut self) {
        let texts = self.settings.language.texts();
        if self.generating() {
            self.set_current_notice(texts.notice_generating.to_owned());
            return;
        }
        let text = self.editor.text().to_owned();
        if text.trim().is_empty() {
            return;
        }
        match text.trim() {
            "/models" => {
                self.editor.clear();
                let active = self
                    .open_session()
                    .default_model
                    .as_ref()
                    .filter(|selection| self.settings.selection_available(selection))
                    .cloned()
                    .unwrap_or_else(|| self.settings.default_selection());
                self.picker = Some(ModelPicker::new(
                    &self.settings,
                    active,
                    PickerMode::SessionDefault,
                ));
                return;
            }
            "/mcp" => {
                self.editor.clear();
                let mut modal = SettingsUi::new(self.settings.clone());
                modal.category = SettingsCategory::Mcp;
                modal.pane = SettingsPane::Content;
                modal.mcp_statuses = self.mcp_registry.snapshots();
                self.modal = Some(modal);
                return;
            }
            "/compact" => {
                self.editor.clear();
                self.open_compaction_preview();
                return;
            }
            "/undo-compact" => {
                self.editor.clear();
                let session_id = self.open_session().id.clone();
                if self.open_session().compactions.is_empty() {
                    self.set_current_notice(
                        self.settings.language.notice_nothing_to_undo().to_owned(),
                    );
                    return;
                }
                match self.mutate_session_and_save(&session_id, |session| {
                    let _ = undo_last_compaction(session);
                }) {
                    Ok(()) => self.set_current_notice(
                        self.settings.language.notice_compaction_undone().to_owned(),
                    ),
                    Err(err) => self.set_current_notice(
                        self.settings.language.notice_session_storage_failed(&err),
                    ),
                }
                return;
            }
            _ => {}
        }
        self.submit_resolved(text);
    }

    fn submit_resolved(&mut self, text: String) {
        let selection = self.effective_selection();
        let provider = self
            .settings
            .provider_by_id(&selection.provider_id)
            .unwrap_or_else(|| self.settings.provider())
            .clone();
        let model_settings = provider.settings_for_model(&selection.model);
        match self.start_turn(text, selection, provider, model_settings, true) {
            Ok(()) => {
                self.editor.clear();
                // 只有请求已完成本地校验并真正启动后，才消费单轮覆盖。
                self.next_turn_model = None;
            }
            Err(runtime_error) => {
                let summary = self.settings.language.runtime_error_notice(&runtime_error);
                let session_id = self.open_session().id.clone();
                self.set_error_notice(session_id, summary, runtime_error);
            }
        }
    }

    fn start_turn(
        &mut self,
        text: String,
        selection: ModelSelection,
        provider: Provider,
        model_settings: ModelSettings,
        allow_auto_compact: bool,
    ) -> Result<(), RuntimeError> {
        if self.generating() {
            return Err(local_runtime_error(
                RuntimeErrorKind::Configuration,
                "another generation is already active",
            ));
        }
        let resolved_api_key = provider.resolved_api_key().map_err(|error| {
            local_runtime_error(RuntimeErrorKind::Configuration, &error.to_string())
        })?;
        if resolved_api_key.is_none() && !provider.has_auth_header() && provider.is_official() {
            return Err(local_runtime_error(
                RuntimeErrorKind::Authentication,
                &self.settings.language.notice_no_api_key(provider.api_kind),
            ));
        }

        self.clear_current_notice();
        let mut working_session = self.open_session().clone();
        let prospective =
            Message::user_with_request(text, selection.clone(), model_settings.parameters.clone());
        let mut prospective_messages = working_session.messages.clone();
        prospective_messages.push(prospective.clone());
        let initial_budget = build_budget(
            &model_settings,
            &self.settings.context,
            ContextInputs {
                system_instructions: &[],
                summary: working_session.summary.as_ref(),
                messages: &prospective_messages,
            },
        );
        let mut auto_compaction = None;
        if allow_auto_compact
            && initial_budget.should_auto_compact(&self.settings.context)
            && let Some(plan) = prepare_compaction(
                &working_session,
                &model_settings,
                &self.settings.context,
                true,
            )
        {
            let archived = plan.archive_count;
            plan.apply(&mut working_session);
            auto_compaction = Some(archived);
            prospective_messages = working_session.messages.clone();
            prospective_messages.push(prospective.clone());
        }

        let budget = build_budget(
            &model_settings,
            &self.settings.context,
            ContextInputs {
                system_instructions: &[],
                summary: working_session.summary.as_ref(),
                messages: &prospective_messages,
            },
        );
        if let Err(error) = llm::validate_request(
            &provider,
            &selection.model,
            &model_settings,
            &budget,
            RequestFeatures::text_stream(),
        ) {
            return Err(RuntimeError::from_report(&error, format!("{error:#}")));
        }

        self.chat_view.borrow_mut().follow_latest();
        let session_id = working_session.id.clone();
        working_session.messages.push(prospective);
        let history = working_session.messages.clone();
        working_session
            .messages
            .push(Message::assistant_streaming_with_request(
                selection.clone(),
                model_settings.parameters.clone(),
            ));
        working_session.updated_at = Utc::now();
        let compacted_context = working_session
            .summary
            .as_ref()
            .map(|summary| summary.context_text());
        if let Err(error) = working_session.save() {
            return Err(local_runtime_error(
                RuntimeErrorKind::Configuration,
                &format!("session storage failed: {error}"),
            ));
        }
        self.sessions[self.current] = working_session;

        let stream_id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.wrapping_add(1);
        let cancellation = CancellationToken::new();
        let model = selection.model.clone();
        self.stream = Some(StreamState {
            id: stream_id,
            session_id: session_id.clone(),
            started: Instant::now(),
            chars: 0,
            done: false,
            aborted: false,
            usage: None,
            stop_reason: None,
            metrics: None,
            finished_at: None,
            provider: provider.clone(),
            proxy: self.settings.proxy.clone(),
            model: model.clone(),
            selection,
            language: self.settings.language,
            cancellation: cancellation.clone(),
            dirty: false,
            last_checkpoint: Instant::now(),
        });
        let sender = self.events.sender();
        self.stream_task = Some(llm::spawn_stream_reply(
            provider,
            self.settings.proxy.clone(),
            model,
            model_settings,
            self.settings.context.clone(),
            compacted_context,
            history,
            self.mcp_registry.clone(),
            stream_id,
            cancellation,
            sender,
        ));
        if let Some(archived) = auto_compaction {
            self.set_notice(
                session_id,
                self.settings.language.notice_compacted(archived, true),
            );
        }
        Ok(())
    }

    /// 中止生成，保留已生成的部分内容。
    fn abort_stream(&mut self, reason: CancellationReason) -> Option<String> {
        let state = self.stream.as_mut().filter(|state| state.generating())?;
        state.cancellation.cancel_with(reason);
        // 丢弃 handle 只表示不再等待；任务本身由 CancellationToken 主动结束并释放连接。
        self.stream_task.take();
        state.aborted = true;
        state.done = true;
        state.finished_at = Some(Instant::now());
        tracing::info!(
            stream = state.id,
            session = ?state.session_id,
            provider = ?state.provider.name,
            model = ?state.model,
            cancel_reason = reason.as_str(),
            "已请求取消流式回复"
        );
        let context = GenerationContext {
            session_id: state.session_id.clone(),
            provider: state.provider.clone(),
            proxy: state.proxy.clone(),
            model: state.model.clone(),
            language: state.language,
        };
        let session_id = context.session_id.clone();
        self.set_stream_message_status(&session_id, MessageStatus::Cancelled, None);
        self.finalize_exchange(context);
        self.open_next_recovery();
        Some(session_id)
    }

    /// 切换到指定 id 的会话。
    ///
    /// 按 id 而非下标定位，避免后台生成导致的列表重排使下标失效。
    fn switch_session(&mut self, id: &str) {
        if let Some(index) = self.sessions.iter().position(|s| s.id == id) {
            self.current = index;
            self.activity_overlay = None;
            self.chat_view.borrow_mut().follow_latest();
            self.focus = Focus::Input;
            self.error_detail = None;
        }
    }

    /// 新建会话并切换过去。
    fn new_session(&mut self) {
        if self.generating() {
            let _ = self.abort_stream(CancellationReason::SessionChanged);
        }
        let session = Session::new();
        let save_error = session.save().err();
        self.sessions.insert(0, session);
        self.current = 0;
        self.sidebar_pos = 0;
        self.chat_view.borrow_mut().follow_latest();
        self.focus = Focus::Input;
        if let Some(err) = save_error {
            let session_id = self.open_session().id.clone();
            self.report_session_storage_error(&session_id, &err);
        }
    }

    /// 删除指定 id 的会话（Settings 入口不可删，调用方已排除）。
    fn delete_session(&mut self, id: &str) {
        if self.session_generating(id) {
            let _ = self.abort_stream(CancellationReason::SessionRemoved);
        }
        // abort 可能重排列表，删除前重新按 id 定位
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else {
            return;
        };
        if let Err(err) = self.sessions[index].delete_file() {
            self.report_session_storage_error(id, &err);
            return;
        }
        self.sessions.remove(index);
        self.notices.remove(id);
        self.activity_overlay = None;
        if self
            .error_detail
            .as_ref()
            .is_some_and(|detail| detail.session_id == id)
        {
            self.error_detail = None;
        }
        if self.sessions.is_empty() {
            // 始终保留至少一个会话
            let session = Session::new();
            let save_error = session.save().err();
            self.sessions.push(session);
            self.current = 0;
            if let Some(err) = save_error {
                let session_id = self.open_session().id.clone();
                self.report_session_storage_error(&session_id, &err);
            }
        } else if index < self.current {
            self.current -= 1;
        } else {
            self.current = self.current.min(self.sessions.len() - 1);
        }
        self.sidebar_pos = self.sidebar_pos.min(self.sessions.len().saturating_sub(1));
        self.chat_view.borrow_mut().follow_latest();
    }

    /// Mouse hit targets come from the last frame, so scrolling and overlays cannot click through.
    pub(crate) fn handle_mouse_events(&mut self, event: MouseEvent) -> bool {
        if matches!(event.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.mouse.borrow_mut().dragging_input = false;
            return false;
        }
        if !matches!(
            event.kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        ) {
            return false;
        }
        let dragging = event.kind == MouseEventKind::Drag(MouseButton::Left);
        let hit = {
            let mouse = self.mouse.borrow();
            if dragging {
                if !mouse.dragging_input {
                    return false;
                }
                mouse.input_area().map(|area| (area, MouseTarget::Input))
            } else {
                mouse.hit(Position::new(event.column, event.row))
            }
        };
        if matches!(event.kind, MouseEventKind::Down(_)) {
            self.mouse.borrow_mut().dragging_input = false;
        }
        let Some((area, target)) = hit else {
            return false;
        };
        match target {
            target @ (MouseTarget::ActivityList
            | MouseTarget::ActivityItem(_)
            | MouseTarget::ActivityDetail
            | MouseTarget::ActivityTab(_)
            | MouseTarget::ActivityBack
            | MouseTarget::ActivityClose) => {
                return ui::activity::handle_mouse(self, target, event.kind);
            }
            MouseTarget::Input
                if dragging || event.kind == MouseEventKind::Down(MouseButton::Left) =>
            {
                self.focus = Focus::Input;
                self.mouse.borrow_mut().dragging_input = true;
                self.editor.move_to_position(
                    area.width as usize,
                    area.height as usize,
                    event
                        .row
                        .clamp(area.y, area.bottom() - 1)
                        .saturating_sub(area.y) as usize,
                    event
                        .column
                        .clamp(area.x, area.right() - 1)
                        .saturating_sub(area.x) as usize,
                    dragging || event.modifiers.contains(KeyModifiers::SHIFT),
                );
            }
            MouseTarget::Activity(key) if event.kind == MouseEventKind::Down(MouseButton::Left) => {
                let mut overlay = ActivityOverlay::new(self.open_session(), Some(key));
                overlay.detail_focus = true;
                self.activity_overlay = Some(overlay);
            }
            MouseTarget::Messages | MouseTarget::Activity(_) => match event.kind {
                MouseEventKind::ScrollUp => self.chat_view.borrow_mut().scroll_by(-3),
                MouseEventKind::ScrollDown => self.chat_view.borrow_mut().scroll_by(3),
                _ => return false,
            },
            MouseTarget::Session(id) if event.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.sessions.iter().position(|session| session.id == id) {
                    self.sidebar_pos = index;
                    self.switch_session(&id);
                }
            }
            MouseTarget::Sessions | MouseTarget::Session(_) => {
                self.focus = Focus::Sidebar;
                match event.kind {
                    MouseEventKind::ScrollUp => {
                        self.sidebar_pos = self.sidebar_pos.saturating_sub(3);
                    }
                    MouseEventKind::ScrollDown => {
                        self.sidebar_pos =
                            (self.sidebar_pos + 3).min(self.sessions.len().saturating_sub(1));
                    }
                    MouseEventKind::Down(MouseButton::Left) => {}
                    _ => return false,
                }
            }
            MouseTarget::Settings if event.kind == MouseEventKind::Down(MouseButton::Left) => {
                self.handle_sidebar_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
            }
            _ => return false,
        }
        true
    }

    /// 输入框焦点下的按键。
    fn handle_input_key(&mut self, key_event: KeyEvent) {
        let bindings = self.settings.keybindings.clone();
        let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key_event.modifiers.contains(KeyModifiers::SHIFT);
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        if bindings.submit.matches(key_event) {
            self.submit();
            return;
        }
        if bindings.newline.matches(key_event) {
            self.editor.insert_newline();
            return;
        }
        if bindings.history_previous.matches(key_event) {
            let history = self.input_history();
            self.editor.navigate_history(&history, true);
            return;
        }
        if bindings.history_next.matches(key_event) {
            let history = self.input_history();
            self.editor.navigate_history(&history, false);
            return;
        }
        if bindings.word_left.matches_with_optional_shift(key_event) {
            self.editor.move_left(true, shift);
            return;
        }
        if bindings.word_right.matches_with_optional_shift(key_event) {
            self.editor.move_right(true, shift);
            return;
        }
        if bindings.select_all.matches(key_event) {
            self.editor.select_all();
            return;
        }
        if bindings.copy.matches(key_event) {
            self.copy_editor_selection();
            return;
        }
        match key_event.code {
            KeyCode::Tab => self.focus_sidebar(),
            KeyCode::Char('p' | 'P') if ctrl => self.focus_sidebar(),
            KeyCode::Esc => {
                self.editor.clear_selection();
            }
            KeyCode::Backspace => self.editor.backspace(ctrl),
            KeyCode::Delete => self.editor.delete(ctrl),
            KeyCode::Left
                if !key_event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT)) =>
            {
                self.editor.move_left(false, shift);
            }
            KeyCode::Right
                if !key_event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT)) =>
            {
                self.editor.move_right(false, shift);
            }
            KeyCode::Up if !key_event.modifiers.contains(KeyModifiers::ALT) => {
                self.editor.move_vertical(-1, shift);
            }
            KeyCode::Down if !key_event.modifiers.contains(KeyModifiers::ALT) => {
                self.editor.move_vertical(1, shift);
            }
            KeyCode::Home => self.editor.move_home(ctrl, shift),
            KeyCode::End => self.editor.move_end(ctrl, shift),
            KeyCode::Char('u' | 'U') if ctrl => {
                self.editor.clear();
            }
            KeyCode::PageUp => {
                self.chat_view.borrow_mut().scroll_by(-8);
            }
            KeyCode::PageDown => {
                self.chat_view.borrow_mut().scroll_by(8);
            }
            KeyCode::Char(c) if plain => {
                let mut encoded = [0u8; 4];
                self.editor.insert_text(c.encode_utf8(&mut encoded));
            }
            _ => {}
        }
    }

    fn input_history(&self) -> Vec<String> {
        self.open_session()
            .messages
            .iter()
            .filter(|message| message.role == Role::User && !message.content().trim().is_empty())
            .map(|message| message.content().clone())
            .collect()
    }

    fn copy_editor_selection(&mut self) {
        let Some(selected) = self.editor.selected_text().map(str::to_owned) else {
            let message = match self.settings.language {
                Lang::Zh => "没有选中的输入文本",
                Lang::En => "No input text is selected",
            };
            self.set_current_notice(message.to_owned());
            return;
        };
        match clipboard::copy(&selected) {
            Ok(()) => {
                let message = match self.settings.language {
                    Lang::Zh => "已复制选中的输入文本",
                    Lang::En => "Selected input copied",
                };
                self.set_current_info(message.to_owned());
            }
            Err(error) => {
                let message = match self.settings.language {
                    Lang::Zh => format!("复制失败：{error}"),
                    Lang::En => format!("Copy failed: {error}"),
                };
                self.set_current_notice(message);
            }
        }
    }

    /// 把焦点切到侧栏并选中当前会话。
    fn focus_sidebar(&mut self) {
        self.focus = Focus::Sidebar;
        self.sidebar_pos = self.current;
    }

    /// 侧栏焦点下的按键。
    ///
    /// ↑/↓ 只在会话列表内移动（不滑入 Settings 入口，避免连按误入设置）；
    /// 设置入口用 `s` 打开。
    fn handle_sidebar_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let last = self.sessions.len().saturating_sub(1); // 最后一条会话
        match key_event.code {
            KeyCode::Tab | KeyCode::Esc => self.focus = Focus::Input,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                self.sidebar_pos = self.sidebar_pos.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                self.sidebar_pos = (self.sidebar_pos + 1).min(last);
            }
            KeyCode::Char('n' | 'N') if plain => self.new_session(),
            KeyCode::Char('r' | 'R') if plain => {
                if let Some(session) = self.sessions.get(self.sidebar_pos) {
                    self.open_rename(session.id.clone());
                }
            }
            KeyCode::Char('/') if plain => self.open_session_search(),
            KeyCode::Char('s' | 'S') if plain => {
                let settings = self.settings.clone();
                let mut modal = SettingsUi::new(settings);
                modal.mcp_statuses = self.mcp_registry.snapshots();
                self.modal = Some(modal);
            }
            KeyCode::Char('d' | 'D') if plain => {
                if let Some(session) = self.sessions.get(self.sidebar_pos) {
                    let id = session.id.clone();
                    self.delete_session(&id);
                }
            }
            KeyCode::Enter => {
                if let Some(session) = self.sessions.get(self.sidebar_pos) {
                    let id = session.id.clone();
                    self.switch_session(&id);
                }
            }
            _ => {}
        }
    }

    // ===== 设置弹窗 =====

    /// 设置弹窗按键总入口。
    fn handle_settings_key(&mut self, key_event: KeyEvent) {
        let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        // Ctrl+S：保存草稿并关闭
        if ctrl
            && matches!(key_event.code, KeyCode::Char('s' | 'S'))
            && self
                .modal
                .as_ref()
                .is_some_and(|modal| modal.keybinding_edit.is_none())
        {
            let valid = if let Some(modal) = self.modal.as_mut() {
                modal.apply_proxy_edit();
                modal.draft.normalize();
                if let Err(error) = modal.draft.keybindings.validate() {
                    modal.validation_error = Some(format!("{error:#}"));
                    modal.category = SettingsCategory::Keyboard;
                    modal.pane = SettingsPane::Content;
                    false
                } else if let Err(error) = modal.draft.validate_mcp_servers() {
                    modal.validation_error = Some(format!("{error:#}"));
                    modal.category = SettingsCategory::Mcp;
                    modal.pane = SettingsPane::Content;
                    false
                } else {
                    match modal.draft.validate() {
                        Ok(()) => true,
                        Err(err) => {
                            modal.validation_error =
                                Some(modal.draft.language.proxy_validation_error(err));
                            modal.category = SettingsCategory::Network;
                            modal.pane = SettingsPane::Content;
                            modal.network_pos = 1;
                            false
                        }
                    }
                }
            } else {
                false
            };
            if valid {
                let modal = self.modal.take().expect("settings modal checked above");
                let mcp_scope_changed = self.settings.mcp_servers != modal.draft.mcp_servers
                    || self.settings.proxy != modal.draft.proxy;
                self.settings = modal.draft;
                self.sparkle
                    .set_enabled(self.settings.whimsy, Instant::now());
                if mcp_scope_changed {
                    let runtime_mcp_servers = self.settings.runtime_mcp_servers();
                    self.mcp_registry
                        .reconfigure(&runtime_mcp_servers, &self.settings.proxy);
                }
                self.persist_settings();
                self.repair_model_selections();
                self.focus = Focus::Input;
            }
            return;
        }

        // MCP 向导打开时独占。
        if let Some(mut wizard) = self
            .modal
            .as_mut()
            .and_then(|modal| modal.mcp_wizard.take())
        {
            self.handle_mcp_wizard_key(&mut wizard, key_event);
            if wizard.step > 0
                && let Some(modal) = self.modal.as_mut()
            {
                modal.mcp_wizard = Some(wizard);
            }
            return;
        }

        // Provider 向导打开时独占
        if let Some(mut wizard) = self.modal.as_mut().and_then(|modal| modal.wizard.take()) {
            self.handle_wizard_key(&mut wizard, key_event);
            if wizard.step > 0
                && let Some(modal) = self.modal.as_mut()
            {
                modal.wizard = Some(wizard);
            }
            return;
        }

        let Some(modal) = self.modal.as_mut() else {
            return;
        };

        if modal.keybinding_edit.is_some() {
            match key_event.code {
                KeyCode::Enter => {
                    let raw = modal
                        .keybinding_edit
                        .as_ref()
                        .map(LineEdit::text)
                        .unwrap_or_default();
                    match raw.parse::<KeyBinding>() {
                        Ok(binding) => {
                            let field = KeybindingField::from_index(modal.keybinding_pos);
                            let mut candidate = modal.draft.keybindings.clone();
                            field.set(&mut candidate, binding);
                            match candidate.validate() {
                                Ok(()) => {
                                    modal.draft.keybindings = candidate;
                                    modal.keybinding_edit = None;
                                    modal.validation_error = None;
                                }
                                Err(error) => {
                                    modal.validation_error = Some(format!("{error:#}"));
                                }
                            }
                        }
                        Err(error) => modal.validation_error = Some(error),
                    }
                }
                KeyCode::Esc => {
                    modal.keybinding_edit = None;
                    modal.validation_error = None;
                }
                _ => {
                    if modal
                        .keybinding_edit
                        .as_mut()
                        .is_some_and(|edit| edit.accept_key(key_event))
                    {
                        modal.validation_error = None;
                    }
                }
            }
            return;
        }

        // 手动添加模型行激活时独占（模型分类）
        if modal.manual_active {
            match key_event.code {
                KeyCode::Enter => {
                    let name = modal.manual.text().trim().to_owned();
                    if !name.is_empty() {
                        let pos = modal.provider_pos.min(modal.draft.providers.len() - 1);
                        if !modal.draft.providers[pos].models.contains(&name) {
                            modal.draft.providers[pos].models.push(name.clone());
                            if modal.draft.current_provider == pos {
                                modal.draft.model = name;
                            }
                        }
                    }
                    modal.manual.clear();
                    modal.manual_active = false;
                }
                KeyCode::Esc => {
                    modal.manual.clear();
                    modal.manual_active = false;
                }
                _ => {
                    modal.manual.accept_key(key_event);
                }
            }
            return;
        }

        match modal.pane {
            SettingsPane::Category => {
                let close = match key_event.code {
                    KeyCode::Up | KeyCode::Char('k') if plain => {
                        modal.category = modal.category.prev();
                        false
                    }
                    KeyCode::Down | KeyCode::Char('j') if plain => {
                        modal.category = modal.category.next();
                        false
                    }
                    KeyCode::Right | KeyCode::Enter | KeyCode::Tab => {
                        modal.pane = SettingsPane::Content;
                        false
                    }
                    KeyCode::Esc => true,
                    _ => false,
                };
                if close {
                    self.modal = None;
                }
            }
            SettingsPane::Content => match modal.category {
                SettingsCategory::Models => self.handle_models_key(key_event),
                SettingsCategory::Context => self.handle_context_key(key_event),
                SettingsCategory::Mcp => self.handle_mcp_settings_key(key_event),
                SettingsCategory::Network => self.handle_network_key(key_event),
                SettingsCategory::Keyboard => self.handle_keyboard_key(key_event),
                SettingsCategory::Appearance => self.handle_appearance_key(key_event),
            },
        }
    }

    /// 「模型」分类内容区的按键。
    fn handle_models_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        let provider_count = modal.draft.providers.len();

        match key_event.code {
            KeyCode::Left | KeyCode::Esc => modal.pane = SettingsPane::Category,
            KeyCode::Tab => {
                modal.section = match modal.section {
                    ModelsSection::Providers => ModelsSection::ModelsList,
                    ModelsSection::ModelsList => ModelsSection::Providers,
                };
                modal.model_pos = 0;
            }
            KeyCode::Up | KeyCode::Char('k') if plain => match modal.section {
                ModelsSection::Providers => {
                    modal.provider_pos = modal.provider_pos.saturating_sub(1);
                    modal.model_pos = 0;
                }
                ModelsSection::ModelsList => {
                    modal.model_pos = modal.model_pos.saturating_sub(1);
                }
            },
            KeyCode::Down | KeyCode::Char('j') if plain => match modal.section {
                ModelsSection::Providers => {
                    modal.provider_pos = (modal.provider_pos + 1).min(provider_count - 1);
                    modal.model_pos = 0;
                }
                ModelsSection::ModelsList => {
                    let len = modal.viewed_provider().models.len();
                    modal.model_pos = (modal.model_pos + 1).min(len.saturating_sub(1));
                }
            },
            KeyCode::Enter => match modal.section {
                ModelsSection::Providers => {
                    modal.draft.current_provider = modal.provider_pos;
                    let provider = modal.viewed_provider();
                    if !provider.models.is_empty() && !provider.models.contains(&modal.draft.model)
                    {
                        modal.draft.model = provider.models[0].clone();
                    }
                }
                ModelsSection::ModelsList => {
                    let Some(name) = modal.viewed_provider().models.get(modal.model_pos).cloned()
                    else {
                        return;
                    };
                    modal.draft.current_provider = modal.provider_pos;
                    modal.draft.model = name;
                }
            },
            // a 新增供应商：进入向导
            KeyCode::Char('a' | 'A') if plain => {
                self.sync_seq = self.sync_seq.wrapping_add(1);
                let wizard = ProviderWizard::new(self.sync_seq);
                modal.wizard = Some(wizard);
            }
            // e 编辑查看中的供应商：复用向导并预填全部字段
            KeyCode::Char('e' | 'E') if plain => {
                self.sync_seq = self.sync_seq.wrapping_add(1);
                let index = modal.provider_pos.min(provider_count - 1);
                let provider = modal.draft.providers[index].clone();
                modal.wizard = Some(ProviderWizard::editing(self.sync_seq, index, &provider));
            }
            // d 删除查看中的供应商（保留至少一个）
            KeyCode::Char('d' | 'D') if plain && provider_count > 1 => {
                let pos = modal.provider_pos.min(provider_count - 1);
                modal.draft.providers.remove(pos);
                match modal.sync_provider {
                    Some(target) if target == pos => {
                        modal.sync_id = None;
                        modal.sync_provider = None;
                    }
                    Some(target) if target > pos => modal.sync_provider = Some(target - 1),
                    _ => {}
                }
                match modal.sync_error_provider {
                    Some(target) if target == pos => {
                        modal.sync_error = None;
                        modal.sync_error_provider = None;
                    }
                    Some(target) if target > pos => {
                        modal.sync_error_provider = Some(target - 1);
                    }
                    _ => {}
                }
                modal.provider_pos = modal.provider_pos.min(modal.draft.providers.len() - 1);
                modal.model_pos = 0;
                if modal.draft.current_provider >= modal.draft.providers.len() {
                    modal.draft.current_provider = modal.draft.providers.len() - 1;
                } else if pos < modal.draft.current_provider {
                    modal.draft.current_provider -= 1;
                }
                let provider = &modal.draft.providers[modal.draft.current_provider];
                if !provider.models.is_empty() && !provider.models.contains(&modal.draft.model) {
                    modal.draft.model = provider.models[0].clone();
                }
            }
            // s 重新同步查看中的供应商模型列表
            KeyCode::Char('s' | 'S') if plain && modal.sync_id.is_none() => {
                self.sync_seq = self.sync_seq.wrapping_add(1);
                let task_id = self.sync_seq;
                let provider = modal.viewed_provider().clone();
                let proxy = modal.draft.proxy.clone();
                let sender = self.events.sender();
                llm::spawn_fetch_models(provider, proxy, task_id, sender);
                modal.sync_id = Some(task_id);
                modal.sync_provider = Some(modal.provider_pos);
                modal.sync_error = None;
                modal.sync_error_provider = None;
            }
            // n 手动添加模型
            KeyCode::Char('n' | 'N') if plain => {
                modal.manual_active = true;
                modal.manual.clear();
            }
            _ => {}
        }
    }

    /// 「网络」分类内容区的按键。
    fn handle_context_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        match key_event.code {
            KeyCode::Esc => modal.pane = SettingsPane::Category,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                modal.context_pos = modal.context_pos.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                modal.context_pos =
                    (modal.context_pos + 1).min(ContextField::ALL.len().saturating_sub(1));
            }
            KeyCode::Tab => {
                modal.context_pos = (modal.context_pos + 1) % ContextField::ALL.len();
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
                let decrease = key_event.code == KeyCode::Left;
                match ContextField::from_index(modal.context_pos) {
                    ContextField::AutoCompact => {
                        modal.draft.context.auto_compact = !modal.draft.context.auto_compact;
                    }
                    ContextField::Threshold => {
                        let value = modal.draft.context.auto_compact_threshold_percent;
                        modal.draft.context.auto_compact_threshold_percent = if decrease {
                            value.saturating_sub(5).max(50)
                        } else {
                            value.saturating_add(5).min(95)
                        };
                    }
                    ContextField::ReservedOutput => {
                        let value = modal.draft.context.reserved_output_tokens;
                        modal.draft.context.reserved_output_tokens = if decrease {
                            value.saturating_sub(512).max(256)
                        } else {
                            value.saturating_add(512).min(262_144)
                        };
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_mcp_settings_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let mut reconnect_name = None;
        {
            let Some(modal) = self.modal.as_mut() else {
                return;
            };
            let server_count = modal.draft.mcp_servers.len();
            match key_event.code {
                KeyCode::Left | KeyCode::Esc => modal.pane = SettingsPane::Category,
                KeyCode::Up | KeyCode::Char('k') if plain => {
                    modal.mcp_pos = modal.mcp_pos.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') if plain => {
                    modal.mcp_pos = (modal.mcp_pos + 1).min(server_count.saturating_sub(1));
                }
                KeyCode::Tab if server_count > 0 => {
                    modal.mcp_pos = (modal.mcp_pos + 1) % server_count;
                }
                KeyCode::Char('a' | 'A') if plain && server_count < 64 => {
                    modal.mcp_wizard = Some(McpServerWizard::new());
                    modal.validation_error = None;
                }
                KeyCode::Char('e' | 'E') if plain && server_count > 0 => {
                    let index = modal.mcp_pos.min(server_count - 1);
                    modal.mcp_wizard = Some(McpServerWizard::editing(
                        index,
                        &modal.draft.mcp_servers[index],
                    ));
                    modal.validation_error = None;
                }
                KeyCode::Char('d' | 'D') if plain && server_count > 0 => {
                    let index = modal.mcp_pos.min(server_count - 1);
                    let removed = modal.draft.mcp_servers.remove(index);
                    modal.mcp_statuses.remove(&removed.name);
                    modal.mcp_pos = modal
                        .mcp_pos
                        .min(modal.draft.mcp_servers.len().saturating_sub(1));
                    modal.validation_error = None;
                }
                KeyCode::Char(' ') | KeyCode::Enter if server_count > 0 => {
                    let index = modal.mcp_pos.min(server_count - 1);
                    modal.draft.mcp_servers[index].enabled =
                        !modal.draft.mcp_servers[index].enabled;
                    modal.validation_error = None;
                }
                KeyCode::Char('c' | 'C' | 'r' | 'R') if plain && server_count > 0 => {
                    reconnect_name = Some(
                        modal.draft.mcp_servers[modal.mcp_pos.min(server_count - 1)]
                            .name
                            .clone(),
                    );
                }
                _ => {}
            }
        }

        let Some(server_name) = reconnect_name else {
            return;
        };
        let result = self
            .settings
            .mcp_servers
            .iter()
            .find(|server| server.name == server_name)
            .cloned()
            .ok_or_else(|| "Save this MCP Server before connecting".to_owned())
            .and_then(|config| {
                self.mcp_registry
                    .reconnect(config, self.settings.proxy.clone())
            });
        if let Some(modal) = self.modal.as_mut() {
            match result {
                Ok(()) => {
                    modal.mcp_statuses = self.mcp_registry.snapshots();
                    modal.validation_error = None;
                }
                Err(error) => modal.validation_error = Some(error),
            }
        }
    }

    fn handle_mcp_wizard_key(&mut self, wizard: &mut McpServerWizard, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        match key_event.code {
            KeyCode::Tab => wizard.field = (wizard.field + 1) % wizard.field_count(),
            KeyCode::Up if plain => wizard.field = wizard.field.saturating_sub(1),
            KeyCode::Down if plain => {
                wizard.field = (wizard.field + 1).min(wizard.field_count() - 1);
            }
            KeyCode::Esc => wizard.step = 0,
            KeyCode::Enter => match wizard.build_config() {
                Ok(config) => {
                    let Some(modal) = self.modal.as_mut() else {
                        wizard.step = 0;
                        return;
                    };
                    let duplicate =
                        modal
                            .draft
                            .mcp_servers
                            .iter()
                            .enumerate()
                            .any(|(index, server)| {
                                Some(index) != wizard.edit_index && server.name == config.name
                            });
                    if duplicate {
                        wizard.error = Some(format!(
                            "MCP Server name {:?} is already in use",
                            config.name
                        ));
                        return;
                    }
                    let name = config.name.clone();
                    let index = if let Some(index) = wizard.edit_index {
                        let old_name = modal.draft.mcp_servers[index].name.clone();
                        modal.draft.mcp_servers[index] = config;
                        if old_name != name {
                            modal.mcp_statuses.remove(&old_name);
                        }
                        index
                    } else {
                        modal.draft.mcp_servers.push(config);
                        modal.draft.mcp_servers.len() - 1
                    };
                    modal.mcp_pos = index;
                    modal.validation_error = None;
                    wizard.step = 0;
                }
                Err(error) => wizard.error = Some(error),
            },
            _ => {
                let field = wizard.field;
                if wizard.line_mut(field).accept_key(key_event) {
                    wizard.error = None;
                }
            }
        }
    }

    /// 「网络」分类内容区的按键。
    fn handle_network_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let Some(modal) = self.modal.as_mut() else {
            return;
        };

        if modal.proxy_editing {
            match key_event.code {
                KeyCode::Enter => {
                    modal.apply_proxy_edit();
                    match modal.draft.proxy.validated_url() {
                        Ok(_) => {
                            modal.proxy_editing = false;
                            modal.validation_error = None;
                        }
                        Err(err) => {
                            modal.validation_error =
                                Some(modal.draft.language.proxy_validation_error(err));
                        }
                    }
                }
                KeyCode::Esc => {
                    modal.reset_proxy_edit();
                    modal.proxy_editing = false;
                    modal.validation_error = None;
                }
                _ => {
                    if modal.proxy_edit.accept_key(key_event) {
                        modal.validation_error = None;
                    }
                }
            }
            return;
        }

        match key_event.code {
            KeyCode::Left | KeyCode::Esc => modal.pane = SettingsPane::Category,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                modal.network_pos = modal.network_pos.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                modal.network_pos = (modal.network_pos + 1).min(NetworkField::ALL.len() - 1);
            }
            KeyCode::Tab => {
                modal.network_pos = (modal.network_pos + 1) % NetworkField::ALL.len();
            }
            KeyCode::Enter => match NetworkField::from_index(modal.network_pos) {
                NetworkField::ProxyMode => {
                    modal.draft.proxy.mode = modal.draft.proxy.mode.next();
                    modal.validation_error = None;
                }
                NetworkField::ProxyUrl => {
                    modal.reset_proxy_edit();
                    modal.proxy_editing = true;
                    modal.validation_error = None;
                }
            },
            _ => {}
        }
    }

    /// 「键盘」分类内容区的按键。
    fn handle_keyboard_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        match key_event.code {
            KeyCode::Left | KeyCode::Esc => modal.pane = SettingsPane::Category,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                modal.keybinding_pos = modal.keybinding_pos.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                modal.keybinding_pos =
                    (modal.keybinding_pos + 1).min(KeybindingField::ALL.len() - 1);
            }
            KeyCode::Tab => {
                modal.keybinding_pos = (modal.keybinding_pos + 1) % KeybindingField::ALL.len();
            }
            KeyCode::Enter => {
                let field = KeybindingField::from_index(modal.keybinding_pos);
                modal.keybinding_edit = Some(LineEdit::from_text(
                    &field.binding(&modal.draft.keybindings).to_string(),
                ));
                modal.validation_error = None;
            }
            _ => {}
        }
    }

    /// 「外观」分类内容区的按键。
    fn handle_appearance_key(&mut self, key_event: KeyEvent) {
        let plain = !key_event
            .modifiers
            .intersects(KeyModifiers::CONTROL.union(KeyModifiers::ALT));
        let Some(modal) = self.modal.as_mut() else {
            return;
        };
        match key_event.code {
            KeyCode::Left | KeyCode::Esc => modal.pane = SettingsPane::Category,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                modal.appearance_pos = modal.appearance_pos.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                modal.appearance_pos = (modal.appearance_pos + 1).min(SettingsField::ALL.len() - 1);
            }
            KeyCode::Tab => {
                modal.appearance_pos = (modal.appearance_pos + 1) % SettingsField::ALL.len();
            }
            KeyCode::Enter => match SettingsField::from_index(modal.appearance_pos) {
                SettingsField::Language => modal.draft.language = modal.draft.language.toggle(),
                SettingsField::Theme => modal.draft.theme = modal.draft.theme.next(),
                SettingsField::TitanArt => modal.draft.show_titan = !modal.draft.show_titan,
                SettingsField::Whimsy => modal.draft.whimsy = !modal.draft.whimsy,
            },
            _ => {}
        }
    }

    /// 新增供应商向导的按键。`wizard.step == 0` 表示向导结束（保存或取消）。
    fn handle_wizard_key(&mut self, wizard: &mut ProviderWizard, key_event: KeyEvent) {
        let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
        let no_ctrl = !ctrl && !key_event.modifiers.contains(KeyModifiers::ALT);

        match wizard.step {
            // 1/3 选择协议
            1 => match key_event.code {
                KeyCode::Up | KeyCode::Char('k') if no_ctrl => {
                    wizard.kind_pos = wizard.kind_pos.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') if no_ctrl => {
                    wizard.kind_pos = (wizard.kind_pos + 1).min(ApiKind::ALL.len() - 1);
                }
                KeyCode::Enter => wizard.step = 2,
                KeyCode::Esc => wizard.step = 0,
                _ => {}
            },
            // 2/3 连接配置（字段间只用 Tab/↑/↓ 切换：文本行不能用 j/k，
            // 否则粘贴的 API Key 含 j/k 时会被当作导航键跳到下一字段）
            2 => match key_event.code {
                KeyCode::Tab => wizard.field = (wizard.field + 1) % 4,
                KeyCode::Esc => wizard.step = 1,
                KeyCode::Up if no_ctrl => wizard.field = wizard.field.saturating_sub(1),
                KeyCode::Down if no_ctrl => wizard.field = (wizard.field + 1).min(3),
                KeyCode::Enter => {
                    // 组装 Provider 并发起 /models 同步
                    let provider = match wizard_provider(wizard) {
                        Ok(provider) => provider,
                        Err(err) => {
                            wizard.error = Some(err.to_string());
                            return;
                        }
                    };
                    let proxy = self
                        .modal
                        .as_ref()
                        .map(|modal| modal.draft.proxy.clone())
                        .unwrap_or_default();
                    let sender = self.events.sender();
                    llm::spawn_fetch_models(provider, proxy, wizard.id, sender);
                    wizard.fetching = true;
                    wizard.error = None;
                    if !wizard.is_editing() {
                        wizard.fetched = None;
                        wizard.selected.clear();
                    }
                    wizard.step = 3;
                }
                _ => {
                    if wizard.line_mut(wizard.field).accept_key(key_event) {
                        wizard.error = None;
                    }
                }
            },
            // 3/3 选择模型
            _ => {
                // 手动添加行激活时独占
                if wizard.manual_active {
                    match key_event.code {
                        KeyCode::Enter => {
                            let name = wizard.manual.text().trim().to_owned();
                            if !name.is_empty() {
                                let list = wizard.fetched.get_or_insert_with(Vec::new);
                                if let Some(pos) = list.iter().position(|m| *m == name) {
                                    if let Some(selected) = wizard.selected.get_mut(pos) {
                                        *selected = true;
                                    }
                                } else {
                                    list.push(name);
                                    wizard.selected.push(true);
                                }
                                wizard.error = None;
                                wizard.warned = false;
                            }
                            wizard.manual.clear();
                            wizard.manual_active = false;
                        }
                        KeyCode::Esc => {
                            wizard.manual.clear();
                            wizard.manual_active = false;
                        }
                        _ => {
                            wizard.manual.accept_key(key_event);
                        }
                    }
                    return;
                }
                if wizard.fetching {
                    // 同步中只允许取消
                    if key_event.code == KeyCode::Esc {
                        wizard.step = 0;
                    }
                    return;
                }
                let Some(list) = wizard.fetched.clone() else {
                    // 尚无结果（同步失败）：r 重试 / n 手动 / Esc 返回
                    match key_event.code {
                        KeyCode::Char('r' | 'R') if no_ctrl => {
                            let provider = match wizard_provider(wizard) {
                                Ok(provider) => provider,
                                Err(err) => {
                                    wizard.error = Some(err.to_string());
                                    return;
                                }
                            };
                            let proxy = self
                                .modal
                                .as_ref()
                                .map(|modal| modal.draft.proxy.clone())
                                .unwrap_or_default();
                            let sender = self.events.sender();
                            llm::spawn_fetch_models(provider, proxy, wizard.id, sender);
                            wizard.fetching = true;
                        }
                        KeyCode::Char('n' | 'N') if no_ctrl => wizard.manual_active = true,
                        KeyCode::Esc => wizard.step = 2,
                        _ => {}
                    }
                    return;
                };
                match key_event.code {
                    KeyCode::Esc => wizard.step = 2,
                    KeyCode::Up | KeyCode::Char('k') if no_ctrl => {
                        wizard.list_pos = wizard.list_pos.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') if no_ctrl => {
                        wizard.list_pos = (wizard.list_pos + 1).min(list.len().saturating_sub(1));
                    }
                    KeyCode::Char(' ') if no_ctrl => {
                        if let Some(flag) = wizard.selected.get_mut(wizard.list_pos) {
                            *flag = !*flag;
                            wizard.warned = false;
                        }
                    }
                    KeyCode::Char('a' | 'A') if no_ctrl => {
                        // 有任一未选 → 全选；全部已选 → 全不选
                        let target = wizard.selected.iter().any(|flag| !*flag);
                        for flag in wizard.selected.iter_mut() {
                            *flag = target;
                        }
                        wizard.warned = false;
                    }
                    KeyCode::Char('n' | 'N') if no_ctrl => wizard.manual_active = true,
                    KeyCode::Char('r' | 'R') if no_ctrl && wizard.error.is_some() => {
                        let provider = match wizard_provider(wizard) {
                            Ok(provider) => provider,
                            Err(err) => {
                                wizard.error = Some(err.to_string());
                                return;
                            }
                        };
                        let proxy = self
                            .modal
                            .as_ref()
                            .map(|modal| modal.draft.proxy.clone())
                            .unwrap_or_default();
                        let sender = self.events.sender();
                        llm::spawn_fetch_models(provider, proxy, wizard.id, sender);
                        wizard.fetching = true;
                        wizard.error = None;
                    }
                    KeyCode::Enter => {
                        let picked: Vec<String> = list
                            .iter()
                            .zip(wizard.selected.iter())
                            .filter(|(_, sel)| **sel)
                            .map(|(name, _)| name.clone())
                            .collect();
                        if picked.is_empty() {
                            wizard.error = None;
                            wizard.warned = true;
                            return;
                        }
                        let provider = match wizard_provider(wizard) {
                            Ok(provider) => provider,
                            Err(err) => {
                                wizard.error = Some(err.to_string());
                                return;
                            }
                        };
                        let Some(modal) = self.modal.as_mut() else {
                            wizard.step = 0;
                            return;
                        };
                        let mut provider = provider;
                        provider.models = picked.clone();
                        provider
                            .model_settings
                            .retain(|model, _| picked.contains(model));
                        let index = if let Some(index) = wizard
                            .edit_index
                            .filter(|index| *index < modal.draft.providers.len())
                        {
                            modal.draft.providers[index] = provider;
                            index
                        } else {
                            modal.draft.providers.push(provider);
                            modal.draft.providers.len() - 1
                        };
                        modal.provider_pos = index;
                        modal.model_pos = 0;
                        if wizard.edit_index.is_none() {
                            modal.draft.current_provider = index;
                            modal.draft.model = picked[0].clone();
                        } else if modal.draft.current_provider == index
                            && !picked.contains(&modal.draft.model)
                        {
                            modal.draft.model = picked[0].clone();
                        }
                        wizard.step = 0;
                    }
                    _ => {}
                }
            }
        }
    }

    /// 模型切换弹窗的按键。
    fn handle_picker_key(&mut self, key_event: KeyEvent) {
        let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key_event.modifiers.contains(KeyModifiers::ALT);
        let plain = !ctrl && !alt;
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        let mut close = false;

        if picker.editing {
            match key_event.code {
                KeyCode::Enter => {
                    let name = picker.buffer.iter().collect::<String>().trim().to_owned();
                    if !name.is_empty() {
                        let idx = picker.provider_idx.min(self.settings.providers.len() - 1);
                        if !self.settings.providers[idx].models.contains(&name) {
                            self.settings.providers[idx].models.push(name.clone());
                        }
                        self.persist_settings();
                        let selection = self.settings.providers[idx].selection(name);
                        self.apply_picker_selection(picker.mode, selection);
                    }
                    close = true;
                }
                KeyCode::Esc => {
                    picker.editing = false;
                    picker.buffer.clear();
                    picker.cursor = 0;
                }
                KeyCode::Backspace => {
                    if picker.cursor > 0 {
                        picker.cursor -= 1;
                        picker.buffer.remove(picker.cursor);
                    }
                }
                KeyCode::Delete => {
                    if picker.cursor < picker.buffer.len() {
                        picker.buffer.remove(picker.cursor);
                    }
                }
                KeyCode::Left => picker.cursor = picker.cursor.saturating_sub(1),
                KeyCode::Right => {
                    picker.cursor = (picker.cursor + 1).min(picker.buffer.len());
                }
                KeyCode::Home => picker.cursor = 0,
                KeyCode::End => picker.cursor = picker.buffer.len(),
                KeyCode::Char('u' | 'U') if ctrl => {
                    picker.buffer.clear();
                    picker.cursor = 0;
                }
                KeyCode::Char(c) if plain => {
                    picker.buffer.insert(picker.cursor, c);
                    picker.cursor += 1;
                }
                _ => {}
            }
        } else {
            let len = self.settings.providers[picker.provider_idx].models.len();
            match key_event.code {
                KeyCode::Esc => close = true,
                KeyCode::Tab | KeyCode::Right if plain => {
                    picker.switch_provider(&self.settings, true);
                }
                KeyCode::BackTab | KeyCode::Left if plain => {
                    picker.switch_provider(&self.settings, false);
                }
                KeyCode::Up | KeyCode::Char('k') if plain => {
                    picker.move_model(&self.settings, false);
                }
                KeyCode::Down | KeyCode::Char('j') if plain => {
                    picker.move_model(&self.settings, true);
                }
                KeyCode::Enter => {
                    if let Some(name) = self.settings.providers[picker.provider_idx]
                        .models
                        .get(picker.selected)
                        .cloned()
                    {
                        let selection =
                            self.settings.providers[picker.provider_idx].selection(name);
                        self.apply_picker_selection(picker.mode, selection);
                    }
                    close = true;
                }
                KeyCode::Char('a' | 'A') if plain => {
                    picker.editing = true;
                    picker.buffer.clear();
                    picker.cursor = 0;
                }
                KeyCode::Char('d' | 'D') if plain && len > 0 => {
                    let idx = picker.provider_idx;
                    self.settings.providers[idx].models.remove(picker.selected);
                    let remaining = self.settings.providers[idx].models.clone();
                    self.settings.providers[idx]
                        .model_settings
                        .retain(|model, _| remaining.contains(model));
                    if self.settings.current_provider == idx {
                        let provider = &self.settings.providers[idx];
                        if !provider.models.is_empty()
                            && !provider.models.contains(&self.settings.model)
                        {
                            self.settings.model = provider.models[0].clone();
                        }
                    }
                    picker.selected = picker.selected.min(len.saturating_sub(2));
                    self.persist_settings();
                    self.repair_model_selections();
                }
                _ => {}
            }
        }

        if !close {
            self.picker = Some(picker);
        }
    }

    fn apply_picker_selection(&mut self, mode: PickerMode, selection: ModelSelection) {
        match mode {
            PickerMode::SessionDefault => {
                let session = &mut self.sessions[self.current];
                session.default_model = Some(selection);
                session.updated_at = Utc::now();
                if let Err(err) = session.save() {
                    let session_id = session.id.clone();
                    self.report_session_storage_error(&session_id, &err);
                }
            }
            PickerMode::NextTurn => self.next_turn_model = Some(selection),
        }
    }

    /// Provider/模型被删除后只清理默认选择；历史消息上的实际模型快照必须保留。
    fn repair_model_selections(&mut self) {
        if self
            .next_turn_model
            .as_ref()
            .is_some_and(|selection| !self.settings.selection_available(selection))
        {
            self.next_turn_model = None;
        }
        let mut errors = Vec::new();
        for session in &mut self.sessions {
            if session
                .default_model
                .as_ref()
                .is_some_and(|selection| !self.settings.selection_available(selection))
            {
                session.default_model = None;
                if let Err(err) = session.save() {
                    errors.push((session.id.clone(), err));
                }
            }
        }
        for (session_id, err) in errors {
            self.report_session_storage_error(&session_id, &err);
        }
    }

    /// 持久化设置，失败时在聊天区给出提示。
    fn persist_settings(&mut self) {
        self.settings.normalize();
        if let Err(err) = self.settings.save() {
            tracing::error!(error = %format!("{err:#}"), "设置保存失败");
            let message = self.settings.language.notice_save_failed(&err.to_string());
            self.set_current_notice(message);
        }
    }

    /// Set running to false to quit the application.
    pub fn quit(&mut self) {
        if self.generating() {
            let _ = self.abort_stream(CancellationReason::ApplicationExit);
        }
        self.running = false;
    }

    fn set_current_notice(&mut self, message: String) {
        let session_id = self.open_session().id.clone();
        self.set_notice(session_id, message);
    }

    fn set_current_info(&mut self, message: String) {
        let session_id = self.open_session().id.clone();
        self.notices.insert(session_id, Notice::info(message));
    }

    fn clear_current_notice(&mut self) {
        let session_id = self.open_session().id.clone();
        self.notices.remove(&session_id);
        if self
            .error_detail
            .as_ref()
            .is_some_and(|detail| detail.session_id == session_id)
        {
            self.error_detail = None;
        }
    }

    fn set_notice(&mut self, session_id: String, message: String) {
        if self
            .error_detail
            .as_ref()
            .is_some_and(|detail| detail.session_id == session_id)
        {
            self.error_detail = None;
        }
        self.notices.insert(session_id, Notice::warning(message));
    }

    fn set_error_notice(&mut self, session_id: String, summary: String, error: RuntimeError) {
        self.notices
            .insert(session_id, Notice::error(summary, error));
    }

    fn report_session_storage_error(&mut self, session_id: &str, err: &color_eyre::Report) {
        tracing::error!(
            session = ?session_id,
            error = %format!("{err:#}"),
            "会话存储操作失败"
        );
        let message = self
            .settings
            .language
            .notice_session_storage_failed(&err.to_string());
        self.notices
            .entry(session_id.to_owned())
            .and_modify(|existing| {
                existing.summary.push('\n');
                existing.summary.push_str(&message);
            })
            .or_insert_with(|| Notice::warning(message));
    }
}

fn local_runtime_error(kind: RuntimeErrorKind, summary: &str) -> RuntimeError {
    RuntimeError {
        kind,
        summary: summary.to_owned(),
        detail: summary.to_owned(),
        request_id: None,
        retry_after_ms: None,
    }
}

#[derive(Debug)]
struct GenerationContext {
    session_id: String,
    provider: Provider,
    proxy: ProxySettings,
    model: String,
    language: Lang,
}

fn stream_message_mut(session: &mut Session, selection: ModelSelection) -> &mut Message {
    let has_streaming_tail = session.messages.last().is_some_and(|message| {
        message.role == Role::Assistant && message.status == MessageStatus::Streaming
    });
    if !has_streaming_tail {
        session
            .messages
            .push(Message::assistant_streaming(selection));
    }
    session
        .messages
        .last_mut()
        .expect("streaming assistant message was just ensured")
}

/// 由向导当前输入组装一个待同步/待保存的 Provider（模型列表为空）。
fn wizard_provider(wizard: &ProviderWizard) -> color_eyre::Result<Provider> {
    let url = wizard.url.text();
    let base_url = (!url.trim().is_empty()).then(|| url.trim().to_owned());
    let mut name = wizard.name.text().trim().to_owned();
    if name.is_empty() {
        name = base_url
            .as_deref()
            .map(|url| name_from_url(Some(url)))
            .unwrap_or_else(|| wizard.api_kind().default_name().to_owned());
    }
    let key_text = wizard.key.text();
    let headers_text = wizard.headers.text();
    let headers = if headers_text.trim().is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_str::<BTreeMap<String, String>>(headers_text.trim()).map_err(|err| {
            color_eyre::eyre::eyre!("HTTP Headers 必须是字符串 JSON object: {err}")
        })?
    };
    let mut provider = Provider {
        id: wizard.provider_id.clone(),
        name,
        api_kind: wizard.api_kind(),
        base_url,
        api_key: (!key_text.trim().is_empty())
            .then(|| crate::secret::SecretValue::from(key_text.trim())),
        models: Vec::new(),
        headers,
        model_settings: wizard.fetched_settings.clone(),
    };
    provider.normalize();
    Ok(provider)
}

fn merge_model_catalog(
    mut existing: BTreeMap<String, ModelSettings>,
    catalog: ModelCatalog,
) -> (Vec<String>, BTreeMap<String, ModelSettings>) {
    let mut models = Vec::with_capacity(catalog.len());
    let mut settings = BTreeMap::new();
    for info in catalog {
        let id = info.id.trim().to_owned();
        if id.is_empty() || models.contains(&id) {
            continue;
        }
        let mut model_settings = existing.remove(&id).unwrap_or_default();
        model_settings.capabilities = info.capabilities;
        settings.insert(id.clone(), model_settings);
        models.push(id);
    }
    (models, settings)
}

fn insert_text(buffer: &mut Vec<char>, cursor: &mut usize, text: &str, multiline: bool) {
    *cursor = (*cursor).min(buffer.len());
    for ch in text.chars() {
        let allowed = !ch.is_control() || (multiline && matches!(ch, '\n' | '\t'));
        if allowed {
            buffer.insert(*cursor, ch);
            *cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str) -> Session {
        let mut session = Session::new();
        session.id = id.to_owned();
        session
    }

    fn test_app() -> App {
        App::new(
            Settings::default(),
            vec![session("session-a/invalid"), session("session-b/invalid")],
        )
    }

    fn start_fake_stream(app: &mut App, session_id: &str, id: u64) {
        let selection = app.settings.default_selection();
        app.stream = Some(StreamState {
            id,
            session_id: session_id.to_owned(),
            started: Instant::now(),
            chars: 0,
            done: false,
            aborted: false,
            usage: None,
            stop_reason: None,
            metrics: None,
            finished_at: None,
            provider: app.settings.provider().clone(),
            proxy: app.settings.proxy.clone(),
            model: app.settings.model.clone(),
            selection,
            language: app.settings.language,
            cancellation: CancellationToken::new(),
            dirty: false,
            last_checkpoint: Instant::now(),
        });
    }

    fn test_metrics(outcome: llm::RequestOutcome) -> RequestMetrics {
        RequestMetrics {
            request_id: Some("req-test".to_owned()),
            total_ms: 10,
            ttft_ms: Some(2),
            output_tokens: Some(1),
            outcome,
            cancellation_reason: None,
        }
    }

    #[tokio::test]
    async fn stream_deltas_stay_with_originating_session() {
        let mut app = test_app();
        start_fake_stream(&mut app, "session-a/invalid", 7);

        app.switch_session("session-b/invalid");
        app.on_delta(7, "reply".to_owned());

        assert_eq!(app.open_session().id, "session-b/invalid");
        assert!(app.open_session().messages.is_empty());
        let source = app
            .sessions
            .iter()
            .find(|session| session.id == "session-a/invalid")
            .unwrap();
        assert_eq!(source.messages[0].role, Role::Assistant);
        assert_eq!(source.messages[0].content(), "reply");
        assert_eq!(
            source.messages[0].model.as_ref().unwrap().model,
            app.settings.model
        );
    }

    #[tokio::test]
    async fn reasoning_and_system_stream_events_stay_structured() {
        let mut app = test_app();
        start_fake_stream(&mut app, "session-a/invalid", 71);
        app.on_reasoning(71, "plan".to_owned());
        app.on_system_event(71, "response.queued".to_owned());

        let message = app.sessions[0].messages.last().unwrap();
        assert!(message.content().is_empty());
        assert!(matches!(
            &message.blocks[0].kind,
            crate::session::BlockKind::Reasoning { content, .. }
                if content == "plan"
        ));
        assert!(matches!(
            &message.blocks[1].kind,
            crate::session::BlockKind::System { message }
                if message == "response.queued"
        ));
    }

    #[tokio::test]
    async fn background_errors_are_visible_only_on_their_session() {
        let mut app = test_app();
        app.sessions[0].messages.push(Message::assistant_streaming(
            app.settings.default_selection(),
        ));
        start_fake_stream(&mut app, "session-a/invalid", 8);
        app.switch_session("session-b/invalid");

        app.on_error(
            8,
            RuntimeError {
                kind: crate::runtime::error::RuntimeErrorKind::Provider,
                summary: "generation failed".to_owned(),
                detail: "generation failed".to_owned(),
                request_id: Some("req-test".to_owned()),
                retry_after_ms: None,
            },
            test_metrics(llm::RequestOutcome::Failed),
        );

        assert!(app.notice().is_none());
        app.switch_session("session-a/invalid");
        assert!(app.notice().unwrap().summary.contains("generation failed"));
        let source = app
            .sessions
            .iter()
            .find(|session| session.id == "session-a/invalid")
            .unwrap();
        assert_eq!(
            source.messages.last().unwrap().status,
            MessageStatus::Failed
        );
        assert!(source.messages.last().unwrap().failure.is_some());
    }

    #[tokio::test]
    async fn ctrl_c_and_escape_cancel_active_generation_before_other_actions() {
        let mut app = test_app();
        start_fake_stream(&mut app, "session-a/invalid", 81);
        app.on_delta(81, "partial".to_owned());
        app.modal = Some(SettingsUi::new(app.settings.clone()));

        app.handle_key_events(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();

        assert!(app.running);
        assert!(app.modal.is_some());
        let state = app.stream.as_ref().unwrap();
        assert!(state.done);
        assert!(state.aborted);
        assert_eq!(state.cancellation.reason(), CancellationReason::User);
        let source = app
            .sessions
            .iter()
            .find(|session| session.id == "session-a/invalid")
            .unwrap();
        assert_eq!(
            source.messages.last().unwrap().status,
            MessageStatus::Cancelled
        );

        start_fake_stream(&mut app, "session-a/invalid", 82);
        app.handle_key_events(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(app.stream.as_ref().unwrap().done);
        assert!(app.modal.is_some());
    }

    #[tokio::test]
    async fn multiline_editor_history_word_selection_and_custom_binding_work_together() {
        let mut app = test_app();
        app.sessions[0]
            .messages
            .push(Message::user("older command".to_owned(), None));
        app.sessions[0]
            .messages
            .push(Message::user("最新问题".to_owned(), None));
        app.editor.set_text("draft");

        app.handle_input_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(app.editor.text(), "最新问题");
        app.handle_input_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.editor.text(), "draft");

        app.editor.set_text("hello 世界");
        app.handle_input_key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
        ));
        assert_eq!(app.editor.selected_text(), Some("世界"));

        app.settings.keybindings.newline = "alt+enter".parse().unwrap();
        app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(app.editor.text(), "hello \n");
    }

    #[tokio::test]
    async fn idle_event_loop_disables_ticks_and_active_work_enables_them() {
        let mut app = test_app();
        assert!(!app.periodic_tick_required());
        app.events
            .set_animation_enabled(app.periodic_tick_required());
        assert!(!app.events.animation_enabled());

        start_fake_stream(&mut app, "session-a/invalid", 83);
        assert!(app.periodic_tick_required());
        app.events
            .set_animation_enabled(app.periodic_tick_required());
        assert!(app.events.animation_enabled());

        app.stream.as_mut().unwrap().done = true;
        app.modal = Some(SettingsUi::new(app.settings.clone()));
        app.modal.as_mut().unwrap().sync_id = Some(1);
        assert!(app.periodic_tick_required());
    }

    #[tokio::test]
    async fn sparkle_survives_typing_paste_and_cursor_movement() {
        use crate::ui::sparkle::render_for_test;
        use crossterm::event::Event as TerminalEvent;
        use std::time::Duration;

        let mut app = App::new(
            Settings {
                whimsy: true,
                ..Settings::default()
            },
            vec![session("sparkle/unsaved")],
        );
        let uninterrupted = Sparkle::new(true);
        let started = Instant::now();
        render_for_test(&app.sparkle, &app.editor, started, true);
        render_for_test(&uninterrupted, &app.editor, started, true);
        let key = |code| TerminalEvent::Key(KeyEvent::new(code, KeyModifiers::NONE));
        for (index, event) in [
            key(KeyCode::Char('q')),
            key(KeyCode::Char('e')),
            key(KeyCode::Left),
            key(KeyCode::Right),
            key(KeyCode::Backspace),
            TerminalEvent::Paste("e  你好\npasted text".to_owned()),
        ]
        .into_iter()
        .enumerate()
        {
            match event {
                TerminalEvent::Key(key) => app.handle_key_events(key).unwrap(),
                TerminalEvent::Paste(text) => app.handle_paste(text),
                _ => unreachable!(),
            }
            assert!(ui::input_is_focused(&app));
            let now = started + Duration::from_secs(2 + index as u64 * 4);
            let expected = render_for_test(&uninterrupted, &app.editor, now, true);
            assert!(expected.content.iter().any(|cell| cell.symbol() != " "));
            assert_eq!(
                render_for_test(&app.sparkle, &app.editor, now, true),
                expected
            );
            assert_eq!(app.sparkle.next_frame(), uninterrupted.next_frame());
        }
        assert_eq!(app.editor.text(), "qe  你好\npasted text");
    }

    #[tokio::test]
    async fn live_whimsy_updates_preserve_other_settings_and_unsaved_appearance_edits() {
        let mut app = test_app();
        let original = app.settings.clone();
        app.modal = Some(SettingsUi::new(app.settings.clone()));
        app.handle_app_event(AppEvent::WhimsyChanged(true));
        assert!(app.settings.whimsy);
        assert!(app.modal.as_ref().unwrap().draft.whimsy);
        assert!(!app.periodic_tick_required());
        assert_eq!(app.ticks, 0);
        let mut expected = original;
        expected.whimsy = true;
        assert_eq!(app.settings, expected);

        app.modal.as_mut().unwrap().appearance_pos = SettingsField::ALL.len() - 1;
        app.handle_appearance_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.modal.as_ref().unwrap().draft.whimsy);
        app.handle_app_event(AppEvent::WhimsyChanged(true));
        assert!(!app.modal.as_ref().unwrap().draft.whimsy);
        app.handle_app_event(AppEvent::WhimsyChanged(false));
        assert!(!app.settings.whimsy);
        assert!(!app.modal.as_ref().unwrap().draft.whimsy);
    }

    #[tokio::test]
    async fn keyboard_settings_validate_before_updating_the_draft() {
        let mut app = test_app();
        app.modal = Some(SettingsUi::new(app.settings.clone()));
        {
            let modal = app.modal.as_mut().unwrap();
            modal.category = SettingsCategory::Keyboard;
            modal.pane = SettingsPane::Content;
            modal.keybinding_pos = 1;
        }

        app.handle_keyboard_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.modal.as_mut().unwrap().keybinding_edit = Some(LineEdit::from_text("ctrl+shift+enter"));
        app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.modal
                .as_ref()
                .unwrap()
                .draft
                .keybindings
                .newline
                .to_string(),
            "ctrl+shift+enter"
        );

        app.modal.as_mut().unwrap().keybinding_pos = 7;
        app.handle_keyboard_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.modal.as_mut().unwrap().keybinding_edit = Some(LineEdit::from_text("enter"));
        app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let modal = app.modal.as_ref().unwrap();
        assert!(modal.keybinding_edit.is_some());
        assert!(
            modal
                .validation_error
                .as_deref()
                .is_some_and(|error| error.contains("同时分配"))
        );
    }

    #[tokio::test]
    async fn background_completion_preserves_open_session_and_ignores_duplicates() {
        let mut app = test_app();
        app.sessions[0].title = Some("existing".to_owned());
        app.sessions[0]
            .messages
            .push(Message::user("question".to_owned(), None));
        start_fake_stream(&mut app, "session-a/invalid", 9);
        app.switch_session("session-b/invalid");
        app.on_delta(9, "answer".to_owned());

        app.on_done(
            9,
            Some(Usage {
                total_tokens: 42,
                ..Usage::default()
            }),
            StopReason::Stop,
            test_metrics(llm::RequestOutcome::Completed),
        );
        app.on_done(
            9,
            None,
            StopReason::Stop,
            test_metrics(llm::RequestOutcome::Completed),
        );

        assert_eq!(app.open_session().id, "session-b/invalid");
        let source = app
            .sessions
            .iter()
            .find(|session| session.id == "session-a/invalid")
            .unwrap();
        assert_eq!(source.messages.len(), 2);
        assert_eq!(
            source.messages.last().unwrap().status,
            MessageStatus::Completed
        );
        assert_eq!(app.context_tokens(), 0);
    }

    #[tokio::test]
    async fn background_reordering_preserves_sidebar_selection_by_id() {
        let mut app = test_app();
        app.sessions[1].title = Some("existing".to_owned());
        app.sessions[1]
            .messages
            .push(Message::user("question".to_owned(), None));
        app.focus = Focus::Sidebar;
        app.sidebar_pos = 0;
        start_fake_stream(&mut app, "session-b/invalid", 10);
        app.on_delta(10, "answer".to_owned());

        app.on_done(
            10,
            None,
            StopReason::Stop,
            test_metrics(llm::RequestOutcome::Completed),
        );

        assert_eq!(app.open_session().id, "session-a/invalid");
        assert_eq!(app.sessions[app.sidebar_pos].id, "session-a/invalid");
        assert_eq!(app.sessions[0].id, "session-b/invalid");
    }

    #[tokio::test]
    async fn alt_shortcuts_do_not_trigger_destructive_sidebar_actions() {
        let mut app = test_app();
        app.focus = Focus::Sidebar;
        app.sidebar_pos = 0;

        app.handle_sidebar_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));

        assert_eq!(app.sessions.len(), 2);
    }

    #[tokio::test]
    async fn model_sync_updates_the_provider_that_started_it() {
        let mut app = test_app();
        let mut second = Provider::new(ApiKind::ChatCompletions);
        second.name = "second".to_owned();
        second.base_url = Some("https://example.com/v1".to_owned());
        second.models = vec!["second-old".to_owned()];
        app.settings.providers.push(second);
        app.modal = Some(SettingsUi::new(app.settings.clone()));
        let modal = app.modal.as_mut().unwrap();
        modal.sync_id = Some(11);
        modal.sync_provider = Some(0);
        modal.provider_pos = 1;

        app.on_models_synced(
            11,
            Ok(vec![llm::ModelInfo {
                id: "first-new".to_owned(),
                capabilities: crate::config::ModelCapabilities::default(),
            }]),
        );

        let modal = app.modal.as_ref().unwrap();
        assert_eq!(modal.draft.providers[0].models, ["first-new"]);
        assert_eq!(modal.draft.providers[1].models, ["second-old"]);
    }

    #[tokio::test]
    async fn model_selection_precedence_is_turn_then_session_then_app() {
        let mut app = test_app();
        let mut second = Provider::new(ApiKind::AnthropicMessages);
        second.name = "Anthropic".to_owned();
        second.models = vec!["claude-test".to_owned()];
        let session_selection = second.selection("claude-test");
        app.settings.providers.push(second);
        app.sessions[0].default_model = Some(session_selection.clone());

        assert_eq!(app.effective_selection(), session_selection);

        let turn_selection = app.settings.default_selection();
        app.next_turn_model = Some(turn_selection.clone());
        assert_eq!(app.effective_selection(), turn_selection);

        app.next_turn_model = None;
        app.sessions[0].default_model = None;
        assert_eq!(app.effective_selection(), app.settings.default_selection());
    }

    #[tokio::test]
    async fn model_picker_moves_across_provider_groups() {
        let mut settings = Settings::default();
        settings.providers[0].id = "provider-gemini".to_owned();
        settings.providers[0].name = "Gemini".to_owned();
        settings.providers[0].api_kind = ApiKind::GeminiGenerateContent;
        settings.providers[0].models = vec!["gemini-3.1-pro-preview".to_owned()];
        settings.model = "gemini-3.1-pro-preview".to_owned();

        let mut cerebras = Provider::new(ApiKind::ChatCompletions);
        cerebras.id = "cerebras".to_owned();
        cerebras.name = "Cerebras".to_owned();
        cerebras.models = vec![
            "gemma-4-31b".to_owned(),
            "qwen-3.8-27b".to_owned(),
            "gpt-oss-120b".to_owned(),
        ];
        settings.providers.push(cerebras);

        let mut app = App::new(settings, vec![session("session-a/invalid")]);
        app.handle_key_events(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();

        let picker = app.picker.as_ref().unwrap();
        assert_eq!((picker.provider_idx, picker.selected), (1, 0));

        app.handle_key_events(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let selected = app.open_session().default_model.as_ref().unwrap();
        assert_eq!(selected.provider_id, "cerebras");
        assert_eq!(selected.model, "qwen-3.8-27b");
    }

    #[test]
    fn model_picker_navigation_clamps_at_empty_provider_edges() {
        let mut settings = Settings::default();
        settings.providers[0].models.clear();
        let mut populated = Provider::new(ApiKind::ChatCompletions);
        populated.models = vec!["model-a".to_owned(), "model-b".to_owned()];
        settings.providers.push(populated);
        settings
            .providers
            .push(Provider::new(ApiKind::ChatCompletions));

        let mut picker = ModelPicker::new(
            &settings,
            settings.providers[0].selection("fallback"),
            PickerMode::SessionDefault,
        );
        picker.move_model(&settings, false);
        assert_eq!((picker.provider_idx, picker.selected), (0, 0));

        picker.move_model(&settings, true);
        assert_eq!((picker.provider_idx, picker.selected), (1, 0));
        picker.move_model(&settings, true);
        assert_eq!((picker.provider_idx, picker.selected), (1, 1));
        picker.move_model(&settings, true);
        assert_eq!((picker.provider_idx, picker.selected), (1, 1));

        picker.switch_provider(&settings, true);
        assert_eq!((picker.provider_idx, picker.selected), (2, 0));
        picker.move_model(&settings, true);
        assert_eq!((picker.provider_idx, picker.selected), (2, 0));
        picker.move_model(&settings, false);
        assert_eq!((picker.provider_idx, picker.selected), (1, 1));
    }

    #[tokio::test]
    async fn picker_modes_do_not_mutate_the_application_default() {
        let mut app = test_app();
        let app_default = app.settings.default_selection();
        let session_selection = app.settings.provider().selection("session-model");
        app.settings.providers[0]
            .models
            .push("session-model".to_owned());
        app.apply_picker_selection(PickerMode::SessionDefault, session_selection.clone());
        assert_eq!(app.open_session().default_model, Some(session_selection));
        assert_eq!(app.settings.default_selection(), app_default);

        let next_selection = app.settings.provider().selection("gpt-4o-mini");
        app.apply_picker_selection(PickerMode::NextTurn, next_selection.clone());
        assert_eq!(app.next_turn_model, Some(next_selection));
        assert_eq!(app.settings.default_selection(), app_default);
    }

    #[tokio::test]
    async fn editing_provider_replaces_it_and_repairs_current_model() {
        let mut app = test_app();
        app.settings.providers[0].name = "old".to_owned();
        app.settings.providers[0].base_url = Some("https://old.example/v1".to_owned());
        app.settings.providers[0].api_key = Some(crate::secret::SecretValue::from("old-key"));
        app.modal = Some(SettingsUi::new(app.settings.clone()));
        {
            let modal = app.modal.as_mut().unwrap();
            modal.pane = SettingsPane::Content;
            modal.category = SettingsCategory::Models;
        }

        app.handle_models_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));

        {
            let wizard = app.modal.as_mut().unwrap().wizard.as_mut().unwrap();
            assert_eq!(wizard.edit_index, Some(0));
            assert_eq!(wizard.name.text(), "old");
            assert_eq!(wizard.url.text(), "https://old.example/v1");
            assert_eq!(wizard.key.text(), "old-key");
            wizard.step = 3;
            wizard.name = LineEdit::from_text("updated");
            wizard.url = LineEdit::from_text("https://new.example/v1");
            wizard.fetched = Some(vec!["new-model".to_owned()]);
            wizard.selected = vec![true];
        }

        app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let modal = app.modal.as_ref().unwrap();
        assert!(modal.wizard.is_none());
        assert_eq!(modal.draft.providers.len(), 1);
        assert_eq!(modal.draft.providers[0].name, "updated");
        assert_eq!(
            modal.draft.providers[0].base_url.as_deref(),
            Some("https://new.example/v1")
        );
        assert_eq!(modal.draft.providers[0].models, ["new-model"]);
        assert_eq!(modal.draft.model, "new-model");
    }

    #[tokio::test]
    async fn completed_stream_speed_is_frozen() {
        let mut app = test_app();
        start_fake_stream(&mut app, "session-a/invalid", 13);
        let state = app.stream.as_mut().unwrap();
        state.started = Instant::now() - std::time::Duration::from_secs(2);
        state.finished_at = Some(state.started + std::time::Duration::from_secs(1));
        state.done = true;
        state.chars = 80;
        state.usage = Some(Usage {
            completion_tokens: 20,
            ..Usage::default()
        });

        let first = state.tokens_per_sec().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let second = state.tokens_per_sec().unwrap();

        assert_eq!(first, 20.0);
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn quit_aborts_generation_and_keeps_partial_reply_in_memory() {
        let mut app = test_app();
        app.sessions[0].title = Some("existing".to_owned());
        start_fake_stream(&mut app, "session-a/invalid", 15);
        app.stream_task = Some(tokio::spawn(std::future::pending()));
        app.on_delta(15, "partial".to_owned());

        app.quit();

        assert!(!app.running);
        let state = app.stream.as_ref().unwrap();
        assert!(state.done);
        assert!(state.aborted);
        assert!(app.stream_task.is_none());
        let source = app
            .sessions
            .iter()
            .find(|session| session.id == "session-a/invalid")
            .unwrap();
        assert_eq!(source.messages.last().unwrap().content(), "partial");
    }

    #[test]
    fn line_edit_supports_navigation_and_delete() {
        let mut edit = LineEdit::default();
        edit.insert_text("abc");
        edit.accept_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        edit.accept_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(edit.text(), "ab");
        edit.accept_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        edit.accept_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(edit.text(), "xab");
    }

    #[test]
    fn provider_wizard_requires_string_header_json() {
        let mut wizard = ProviderWizard::new(1);
        wizard.headers = LineEdit::from_text(r#"{"Authorization":42}"#);
        assert!(wizard_provider(&wizard).is_err());

        wizard.headers = LineEdit::from_text(r#"{"Authorization":"Bearer test"}"#);
        let provider = wizard_provider(&wizard).unwrap();
        assert_eq!(
            provider.headers.get("authorization").map(String::as_str),
            Some("Bearer test")
        );
    }

    #[test]
    fn model_catalog_refresh_preserves_user_parameters() {
        let existing = BTreeMap::from([(
            "model".to_owned(),
            ModelSettings {
                parameters: crate::config::ModelParameters {
                    temperature: Some(0.4),
                    ..crate::config::ModelParameters::default()
                },
                ..ModelSettings::default()
            },
        )]);
        let (models, settings) = merge_model_catalog(
            existing,
            vec![llm::ModelInfo {
                id: "model".to_owned(),
                capabilities: crate::config::ModelCapabilities {
                    context_window: Some(32_000),
                    ..crate::config::ModelCapabilities::default()
                },
            }],
        );
        assert_eq!(models, ["model"]);
        assert_eq!(settings["model"].parameters.temperature, Some(0.4));
        assert_eq!(settings["model"].capabilities.context_window, Some(32_000));
    }

    #[tokio::test]
    async fn pasted_multiline_text_is_preserved_for_chat_editor() {
        let mut buffer = Vec::new();
        let mut cursor = 0;
        insert_text(&mut buffer, &mut cursor, "a\nb\t", true);
        assert_eq!(buffer.iter().collect::<String>(), "a\nb\t");

        let mut line = LineEdit::default();
        line.insert_text("a\nb\t");
        assert_eq!(line.text(), "ab");

        let mut app = test_app();
        app.handle_paste("first\r\nsecond\tvalue".to_owned());
        assert_eq!(app.editor.text(), "first\nsecond\tvalue");
    }

    #[tokio::test]
    async fn f3_opens_an_independent_process_browser() {
        let mut app = test_app();
        app.handle_key_events(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
            .unwrap();
        assert!(app.activity_overlay.as_ref().unwrap().selected.is_none());
        app.handle_key_events(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
            .unwrap();

        app.sessions[0].messages.push(Message::assistant_streaming(
            app.settings.default_selection(),
        ));
        app.sessions[0]
            .messages
            .last_mut()
            .unwrap()
            .push_tool_result(
                "call-1".to_owned(),
                "docs".to_owned(),
                "search".to_owned(),
                serde_json::json!({"content": "large result"}),
                false,
            );

        app.handle_key_events(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
            .unwrap();
        assert!(app.activity_overlay.as_ref().unwrap().selected.is_some());
        app.handle_key_events(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
            .unwrap();
        assert!(app.activity_overlay.is_none());
    }

    #[tokio::test]
    async fn paste_does_not_pass_through_a_non_editing_picker() {
        let mut app = test_app();
        app.picker = Some(ModelPicker::new(
            &app.settings,
            app.settings.default_selection(),
            PickerMode::SessionDefault,
        ));

        app.handle_paste("hidden".to_owned());

        assert!(app.editor.is_empty());
    }

    #[tokio::test]
    async fn mcp_settings_keyboard_flow_adds_edits_toggles_connects_and_removes() {
        let mut app = test_app();
        app.modal = Some(SettingsUi::new(app.settings.clone()));
        {
            let modal = app.modal.as_mut().unwrap();
            modal.category = SettingsCategory::Mcp;
            modal.pane = SettingsPane::Content;
        }

        app.handle_settings_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(
            app.modal
                .as_ref()
                .unwrap()
                .mcp_wizard
                .as_ref()
                .unwrap()
                .step,
            1
        );
        {
            let wizard = app.modal.as_mut().unwrap().mcp_wizard.as_mut().unwrap();
            wizard.field = 1;
            wizard.url.clear();
        }
        app.handle_paste("https://example.com/mcp".to_owned());
        assert!(app.editor.is_empty());
        assert_eq!(
            app.modal
                .as_ref()
                .unwrap()
                .mcp_wizard
                .as_ref()
                .unwrap()
                .url
                .text(),
            "https://example.com/mcp"
        );
        app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let modal = app.modal.as_ref().unwrap();
        assert!(modal.mcp_wizard.is_none());
        assert_eq!(modal.draft.mcp_servers.len(), 1);

        app.handle_settings_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!app.modal.as_ref().unwrap().draft.mcp_servers[0].enabled);
        app.handle_settings_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert!(
            app.modal
                .as_ref()
                .unwrap()
                .mcp_wizard
                .as_ref()
                .unwrap()
                .is_editing()
        );
        app.handle_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.modal.as_ref().unwrap().mcp_wizard.is_none());

        app.handle_settings_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(
            app.modal
                .as_ref()
                .unwrap()
                .validation_error
                .as_deref()
                .is_some_and(|error| error.contains("Save this MCP Server"))
        );
        app.handle_settings_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(app.modal.as_ref().unwrap().draft.mcp_servers.is_empty());
    }

    #[tokio::test]
    async fn startup_prioritizes_each_persisted_unfinished_generation_for_recovery() {
        let selection = Settings::default().default_selection();
        let mut first = session("recovery-a-invalid");
        let mut partial = Message::assistant_streaming_with_request(
            selection.clone(),
            crate::config::ModelParameters {
                max_output_tokens: Some(2048),
                ..crate::config::ModelParameters::default()
            },
        );
        partial.append_text("saved partial");
        first.messages.push(partial);
        let mut second = session("recovery-b-invalid");
        second
            .messages
            .push(Message::assistant_streaming(selection));

        let app = App::new(Settings::default(), vec![first, second]);

        assert_eq!(app.current, 0);
        assert!(matches!(
            app.conversation_overlay,
            Some(ConversationOverlay::Recovery(RecoveryState {
                ref session_id,
                message_index: 0,
                selected: 0,
                ..
            })) if session_id == "recovery-a-invalid"
        ));
        assert_eq!(
            app.sessions[0].messages[0]
                .parameters
                .as_ref()
                .and_then(|parameters| parameters.max_output_tokens),
            Some(2048)
        );
    }

    #[tokio::test]
    async fn session_search_returns_title_body_provider_model_parameters_and_status_matches() {
        let mut app = test_app();
        app.sessions[0].title = Some("atomic persistence".to_owned());
        let selection = app.settings.default_selection();
        let mut message = Message::assistant_streaming_with_request(
            selection,
            crate::config::ModelParameters {
                max_output_tokens: Some(7777),
                ..crate::config::ModelParameters::default()
            },
        );
        message.append_text("searchable body");
        message.status = MessageStatus::Failed;
        app.sessions[0].messages.push(message);

        for query in [
            "atomic persistence",
            "searchable body",
            "openai",
            "gpt-4o-mini",
            "7777",
            "failed",
        ] {
            assert_eq!(app.search_results(query), vec![0], "query: {query}");
        }
        assert!(app.search_results("absent").is_empty());
    }

    #[tokio::test]
    async fn slash_models_and_mcp_open_existing_lists_without_sending_messages() {
        let mut app = test_app();
        let original_messages = app.open_session().messages.len();

        app.editor.set_text("/models");
        app.submit();
        assert!(app.editor.text().is_empty());
        assert!(matches!(
            app.picker.as_ref().map(|picker| picker.mode),
            Some(PickerMode::SessionDefault)
        ));
        assert_eq!(app.open_session().messages.len(), original_messages);

        app.picker = None;
        app.editor.set_text("/mcp");
        app.submit();
        assert!(app.editor.text().is_empty());
        assert!(matches!(
            app.modal.as_ref(),
            Some(SettingsUi {
                category: SettingsCategory::Mcp,
                pane: SettingsPane::Content,
                ..
            })
        ));
        assert_eq!(app.open_session().messages.len(), original_messages);
    }
}
