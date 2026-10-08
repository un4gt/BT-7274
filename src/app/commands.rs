//! Local commands never enter the conversation or provider request history.

use std::{cell::Cell, collections::HashMap};

use super::{App, ModelPicker, PickerMode};
use crate::{
    config::{
        ApiKind, CapabilitySupport, ModelSelection, ModelSettings, ReasoningSettings, Settings,
    },
    i18n::Lang,
    runtime::{mcp::McpServerSnapshot, tool::ToolDefinition},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Model,
    Effort,
    Mcp,
    Compact,
    UndoCompact,
}

impl SlashCommand {
    pub const ALL: [Self; 5] = [
        Self::Model,
        Self::Effort,
        Self::Mcp,
        Self::Compact,
        Self::UndoCompact,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Model => "/model",
            Self::Effort => "/effort",
            Self::Mcp => "/mcp",
            Self::Compact => "/compact",
            Self::UndoCompact => "/undo-compact",
        }
    }

    pub fn description(self, language: Lang) -> &'static str {
        match (language, self) {
            (Lang::Zh, Self::Model) => "选择会话模型",
            (Lang::Zh, Self::Effort) => "调整模型思考级别",
            (Lang::Zh, Self::Mcp) => "查看 MCP 连接与可用工具",
            (Lang::Zh, Self::Compact) => "预览上下文压缩",
            (Lang::Zh, Self::UndoCompact) => "撤销最近一次压缩",
            (Lang::En, Self::Model) => "Choose the session model",
            (Lang::En, Self::Effort) => "Adjust model reasoning",
            (Lang::En, Self::Mcp) => "MCP connections and available tools",
            (Lang::En, Self::Compact) => "Preview context compaction",
            (Lang::En, Self::UndoCompact) => "Undo the last compaction",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct CommandState {
    pub selected: Option<SlashCommand>,
    pub dismissed: Option<String>,
    pub overlay: Option<CommandOverlay>,
    pub replies: HashMap<String, CommandReply>,
}

#[derive(Debug)]
pub(crate) struct CommandReply {
    pub command: String,
    pub body: String,
}

#[derive(Debug)]
pub(crate) enum CommandOverlay {
    Effort(EffortPicker),
    Mcp { scroll: Cell<usize> },
}

#[derive(Debug)]
pub(crate) struct EffortPicker {
    pub model: ModelSelection,
    pub options: Vec<Option<ReasoningSettings>>,
    pub active: Option<ReasoningSettings>,
    pub selected: usize,
    pub unsupported: bool,
    pub error: Option<String>,
}

impl EffortPicker {
    fn new(app: &App) -> Self {
        let model = app.effective_selection();
        let provider = app.provider_for_selection(&model);
        let settings = provider.settings_for_model(&model.model);
        let active = settings.parameters.reasoning.clone();
        let options = effort_options(provider.api_kind, &settings);
        let selected = options
            .iter()
            .position(|option| *option == active)
            .unwrap_or(0);
        Self {
            model,
            options,
            active,
            selected,
            unsupported: settings.capabilities.reasoning == CapabilitySupport::Unsupported,
            error: None,
        }
    }

    /// Build and validate before saving, so failed writes leave live settings unchanged.
    fn candidate(&self, current: &Settings) -> Result<Settings, String> {
        if !current.selection_available(&self.model) {
            return Err("Model is no longer available".to_owned());
        }
        let mut candidate = current.clone();
        let provider = candidate
            .providers
            .iter_mut()
            .find(|provider| provider.id == self.model.provider_id)
            .unwrap();
        let model_settings = provider.settings_for_model(&self.model.model);
        let option = self.options.get(self.selected).ok_or("Invalid selection")?;
        if !effort_options(provider.api_kind, &model_settings).contains(option) {
            return Err("Model reasoning settings have changed; reopen /effort".to_owned());
        }
        let parameters = &mut provider
            .model_settings
            .entry(self.model.model.clone())
            .or_default()
            .parameters;
        parameters.reasoning = option.clone();
        // The budget must remain below the output limit after runtime catalog metadata is gone.
        if matches!(option, Some(ReasoningSettings::Anthropic { .. }))
            && parameters.max_output_tokens.is_none()
        {
            parameters.max_output_tokens = model_settings.capabilities.max_output_tokens;
        }
        candidate
            .validate_document()
            .map_err(|error| error.to_string())?;
        Ok(candidate)
    }
}

fn effort_options(kind: ApiKind, model: &ModelSettings) -> Vec<Option<ReasoningSettings>> {
    let mut options = vec![None];
    if model.capabilities.reasoning == CapabilitySupport::Unsupported {
        return options;
    }
    match kind {
        ApiKind::ChatCompletions | ApiKind::Responses => {
            options.extend(["minimal", "low", "medium", "high", "xhigh"].map(|effort| {
                Some(ReasoningSettings::OpenAi {
                    effort: effort.to_owned(),
                })
            }));
        }
        ApiKind::AnthropicMessages => {
            let max_tokens = model
                .parameters
                .max_output_tokens
                .or(model.capabilities.max_output_tokens)
                .unwrap_or(4096);
            options.extend(
                [1024, 2048, 4096, 8192, 16384, 32768]
                    .into_iter()
                    .filter(|budget| *budget < max_tokens)
                    .map(|budget_tokens| Some(ReasoningSettings::Anthropic { budget_tokens })),
            );
        }
        ApiKind::GeminiGenerateContent => {
            options.extend(
                [-1, 0, 1024, 4096, 8192, 16384, 24576]
                    .map(|thinking_budget| Some(ReasoningSettings::Gemini { thinking_budget })),
            );
        }
    }
    // Preserve a configured custom budget rather than silently displaying it as default.
    let current_is_valid = match &model.parameters.reasoning {
        Some(ReasoningSettings::Anthropic { budget_tokens }) => {
            kind == ApiKind::AnthropicMessages
                && *budget_tokens >= 1024
                && *budget_tokens
                    < model
                        .parameters
                        .max_output_tokens
                        .or(model.capabilities.max_output_tokens)
                        .unwrap_or(4096)
        }
        Some(ReasoningSettings::Gemini { thinking_budget }) => {
            kind == ApiKind::GeminiGenerateContent && *thinking_budget >= -1
        }
        _ => false,
    };
    if current_is_valid && !options.contains(&model.parameters.reasoning) {
        options.push(model.parameters.reasoning.clone());
    }
    options
}

pub(crate) fn effort_label(reasoning: Option<&ReasoningSettings>, language: Lang) -> String {
    match reasoning {
        None => match language {
            Lang::Zh => "默认（由模型决定）".to_owned(),
            Lang::En => "Default (model decides)".to_owned(),
        },
        Some(ReasoningSettings::OpenAi { effort }) => effort.clone(),
        Some(ReasoningSettings::Gemini {
            thinking_budget: -1,
        }) => match language {
            Lang::Zh => "自动调整思考预算".to_owned(),
            Lang::En => "Automatic thinking budget".to_owned(),
        },
        Some(ReasoningSettings::Gemini { thinking_budget: 0 }) => match language {
            Lang::Zh => "关闭思考".to_owned(),
            Lang::En => "Thinking off".to_owned(),
        },
        Some(reasoning) => {
            let budget = match reasoning {
                ReasoningSettings::Anthropic { budget_tokens } => budget_tokens.to_string(),
                ReasoningSettings::Gemini { thinking_budget } => thinking_budget.to_string(),
                ReasoningSettings::OpenAi { .. } => unreachable!(),
            };
            match language {
                Lang::Zh => format!("思考预算：{budget} tokens"),
                Lang::En => format!("Thinking budget: {budget} tokens"),
            }
        }
    }
}

impl App {
    pub(crate) fn command_matches(&self) -> Vec<SlashCommand> {
        let text = self.editor.text();
        let query = text.trim_start();
        if self.commands.dismissed.as_deref() == Some(text)
            || !query.starts_with('/')
            || query.chars().any(char::is_whitespace)
            || self.editor.cursor() != text.len()
            || self.editor.selection().is_some()
        {
            return Vec::new();
        }
        SlashCommand::ALL
            .into_iter()
            .filter(|command| command.name().starts_with(query))
            .collect()
    }

    pub(crate) fn command_menu_selection(&self, matches: &[SlashCommand]) -> usize {
        matches
            .iter()
            .position(|command| Some(*command) == self.commands.selected)
            .unwrap_or(0)
    }

    pub(super) fn handle_command_menu_key(&mut self, key: KeyEvent) -> bool {
        if self
            .commands
            .dismissed
            .as_deref()
            .is_some_and(|text| text != self.editor.text())
        {
            self.commands.dismissed = None;
            self.commands.selected = None;
        }
        let matches = self.command_matches();
        if matches.is_empty() {
            return false;
        }
        let selected = self.command_menu_selection(&matches);
        if self.settings.keybindings.submit.matches(key) {
            self.editor.clear();
            self.editor.insert_text(matches[selected].name());
            self.submit();
            return true;
        }
        if self.settings.keybindings.newline.matches(key) {
            return false;
        }
        if key.modifiers != KeyModifiers::NONE {
            return false;
        }
        match key.code {
            KeyCode::Up => self.commands.selected = Some(matches[selected.saturating_sub(1)]),
            KeyCode::Down => {
                self.commands.selected = Some(matches[(selected + 1).min(matches.len() - 1)])
            }
            KeyCode::Tab => {
                self.editor.clear();
                self.editor.insert_text(matches[selected].name());
            }
            KeyCode::Esc => self.commands.dismissed = Some(self.editor.text().to_owned()),
            _ => return false,
        }
        true
    }

    pub(super) fn execute_slash_command(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.contains(['\n', '\r']) {
            return false;
        }
        let mut words = text.split_whitespace();
        let Some(command) = words.next() else {
            return false;
        };
        if matches!(command, "/compact" | "/undo-compact") && words.clone().next().is_none() {
            return false;
        }
        let Some(name) = command.strip_prefix('/') else {
            return false;
        };
        if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '-') {
            return false;
        }
        if !matches!(command, "/model" | "/models" | "/effort" | "/mcp") || words.next().is_some() {
            let body = match self.settings.language {
                Lang::Zh => "未知命令或多余参数。输入 / 选择命令；/model、/effort、/mcp 无需参数。",
                Lang::En => {
                    "Unknown command or extra arguments. Type / to choose; /model, /effort and /mcp take no arguments."
                }
            };
            self.set_command_reply(command, body.to_owned());
            return true;
        }
        self.editor.clear();
        self.commands.dismissed = None;
        self.commands.selected = None;
        match command {
            "/model" | "/models" => {
                let active = self
                    .open_session()
                    .default_model
                    .as_ref()
                    .filter(|selection| self.settings.selection_available(selection))
                    .cloned()
                    .unwrap_or_else(|| self.settings.default_selection());
                let mut picker =
                    ModelPicker::new(&self.settings, active, PickerMode::SessionDefault);
                picker.from_command = true;
                self.picker = Some(picker);
            }
            "/effort" => {
                self.commands.overlay = Some(CommandOverlay::Effort(EffortPicker::new(self)))
            }
            "/mcp" => {
                self.commands.overlay = Some(CommandOverlay::Mcp {
                    scroll: Cell::new(0),
                })
            }
            _ => unreachable!(),
        }
        true
    }

    pub(crate) fn command_reply(&self) -> Option<&CommandReply> {
        self.commands.replies.get(&self.open_session().id)
    }

    fn set_command_reply(&mut self, command: &str, body: String) {
        self.commands.replies.insert(
            self.open_session().id.clone(),
            CommandReply {
                command: command.to_owned(),
                body,
            },
        );
    }

    pub(super) fn model_command_reply(&mut self, selection: &ModelSelection) {
        let label = match self.settings.language {
            Lang::Zh => "会话模型",
            Lang::En => "Session model",
        };
        self.set_command_reply(
            "/model",
            format!("{label}: {} · {}", selection.provider_name, selection.model),
        );
    }

    pub(crate) fn mcp_command_catalog(&self) -> Vec<(McpServerSnapshot, Vec<ToolDefinition>)> {
        self.mcp_registry.catalog_snapshot()
    }

    pub(super) fn handle_command_overlay_key(&mut self, key: KeyEvent) {
        let Some(mut overlay) = self.commands.overlay.take() else {
            return;
        };
        if key.code == KeyCode::Esc {
            return;
        }
        match &mut overlay {
            CommandOverlay::Effort(picker) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    picker.selected = picker.selected.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    picker.selected = (picker.selected + 1).min(picker.options.len() - 1)
                }
                KeyCode::Home => picker.selected = 0,
                KeyCode::End => picker.selected = picker.options.len() - 1,
                KeyCode::Enter => {
                    match picker.candidate(&self.settings).and_then(|candidate| {
                        candidate.save().map_err(|error| {
                            self.settings
                                .language
                                .notice_save_failed(&error.to_string())
                        })?;
                        Ok(candidate)
                    }) {
                        Ok(candidate) => {
                            self.settings = candidate;
                            let value = effort_label(
                                picker.options[picker.selected].as_ref(),
                                self.settings.language,
                            );
                            self.set_command_reply(
                                "/effort",
                                format!(
                                    "{} · {}: {value}",
                                    picker.model.provider_name, picker.model.model
                                ),
                            );
                            return;
                        }
                        Err(error) => picker.error = Some(error),
                    }
                }
                _ => {}
            },
            CommandOverlay::Mcp { scroll } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => scroll.set(scroll.get().saturating_sub(1)),
                KeyCode::Down | KeyCode::Char('j') => scroll.set(scroll.get().saturating_add(1)),
                KeyCode::PageUp => scroll.set(scroll.get().saturating_sub(8)),
                KeyCode::PageDown => scroll.set(scroll.get().saturating_add(8)),
                KeyCode::Home => scroll.set(0),
                KeyCode::End => scroll.set(usize::MAX),
                _ => {}
            },
        }
        self.commands.overlay = Some(overlay);
    }

    pub(super) fn handle_command_overlay_mouse(&mut self, event: MouseEvent) -> bool {
        let key = match event.kind {
            MouseEventKind::ScrollUp => KeyCode::Up,
            MouseEventKind::ScrollDown => KeyCode::Down,
            _ => return false,
        };
        self.handle_command_overlay_key(KeyEvent::new(key, KeyModifiers::NONE));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Focus, config::Provider, session::Session};

    fn app() -> App {
        App::new(Settings::default(), vec![Session::new(), Session::new()])
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key_events(KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap();
    }

    #[tokio::test]
    async fn command_menu_filters_completes_and_opens_the_selected_command() {
        let mut app = app();
        app.editor.set_text("/");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.editor.text(), "/effort");
        assert_eq!(app.focus, Focus::Input);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(
            app.commands.overlay,
            Some(CommandOverlay::Effort(_))
        ));
        assert!(app.editor.is_empty());
        app.handle_paste("must not reach the editor".to_owned());
        assert!(app.editor.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(app.commands.overlay.is_none());

        app.editor.set_text("/m");
        assert_eq!(
            app.command_matches(),
            [SlashCommand::Model, SlashCommand::Mcp]
        );
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(
            app.commands.overlay,
            Some(CommandOverlay::Mcp { .. })
        ));
        assert!(app.open_session().messages.is_empty());
        assert!(app.stream.is_none());
    }

    #[tokio::test]
    async fn command_menu_respects_custom_submit_and_newline_bindings() {
        let mut app = app();
        app.settings.keybindings.submit = "ctrl+enter".parse().unwrap();
        app.settings.keybindings.newline = "enter".parse().unwrap();
        app.editor.set_text("/mcp");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.text(), "/mcp\n");
        assert!(app.commands.overlay.is_none());
        assert!(app.command_matches().is_empty());
        app.editor.set_text("/mc");
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
            .unwrap();
        assert!(matches!(
            app.commands.overlay,
            Some(CommandOverlay::Mcp { .. })
        ));
    }

    #[tokio::test]
    async fn dismissal_and_editor_selection_do_not_execute_commands() {
        let mut app = app();
        app.editor.set_text("/m");
        press(&mut app, KeyCode::Esc);
        assert!(app.command_matches().is_empty());
        assert_eq!(app.editor.text(), "/m");
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.command_matches(), [SlashCommand::Mcp]);
        press(&mut app, KeyCode::Left);
        assert!(app.command_matches().is_empty());
        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(app.command_matches().is_empty());
        assert!(app.open_session().messages.is_empty());
    }

    #[tokio::test]
    async fn unknown_commands_are_local_but_paths_and_multiline_prompts_are_not_commands() {
        let mut app = app();
        for text in [
            "/unknown",
            "/model extra",
            "/effort high",
            "/mcp extra",
            "/compact extra",
        ] {
            app.editor.set_text(text);
            app.submit();
            assert!(app.command_reply().is_some());
            assert_eq!(app.editor.text(), text);
            assert!(app.open_session().messages.is_empty());
            assert!(app.stream.is_none());
        }
        app.current = 1;
        assert!(app.command_reply().is_none());
        app.current = 0;
        app.editor.clear();
        press(&mut app, KeyCode::Esc);
        assert!(app.command_reply().is_none());

        for text in [
            "/tmp/file.rs",
            "/model\nexplain this command",
            "ordinary text",
        ] {
            assert!(!app.execute_slash_command(text));
        }
    }

    #[tokio::test]
    async fn effort_targets_the_effective_model_and_preserves_other_parameters() {
        let mut app = app();
        let mut provider = Provider::new(ApiKind::GeminiGenerateContent);
        provider.models = vec!["gemini-test".into()];
        let selection = provider.selection("gemini-test");
        let model = provider
            .model_settings
            .entry(selection.model.clone())
            .or_default();
        model.parameters.temperature = Some(0.7);
        model.parameters.reasoning = Some(ReasoningSettings::Gemini {
            thinking_budget: 2048,
        });
        app.settings.providers.push(provider);
        app.next_turn_model = Some(selection.clone());
        let mut picker = EffortPicker::new(&app);
        assert_eq!(picker.model, selection);
        assert_eq!(picker.options[picker.selected], picker.active);
        picker.selected = picker
            .options
            .iter()
            .position(|value| {
                *value
                    == Some(ReasoningSettings::Gemini {
                        thinking_budget: 8192,
                    })
            })
            .unwrap();
        let candidate = picker.candidate(&app.settings).unwrap();
        let updated = candidate
            .provider_by_id(&selection.provider_id)
            .unwrap()
            .settings_for_model(&selection.model);
        assert_eq!(updated.parameters.temperature, Some(0.7));
        assert_eq!(
            updated.parameters.reasoning,
            Some(ReasoningSettings::Gemini {
                thinking_budget: 8192
            })
        );
        assert_eq!(candidate.providers[0], app.settings.providers[0]);
        assert_eq!(
            app.settings.providers[1]
                .settings_for_model(&selection.model)
                .parameters
                .reasoning,
            picker.active
        );

        picker.selected = 0;
        let candidate = picker.candidate(&candidate).unwrap();
        assert!(
            candidate.providers[1]
                .settings_for_model(&selection.model)
                .parameters
                .reasoning
                .is_none()
        );
    }

    #[tokio::test]
    async fn effort_budgets_obey_output_limits_and_keep_custom_values() {
        let mut app = app();
        app.settings.providers[0].api_kind = ApiKind::AnthropicMessages;
        let model = app.settings.providers[0]
            .model_settings
            .entry(app.settings.model.clone())
            .or_default();
        model.capabilities.max_output_tokens = Some(10_000);
        model.parameters.reasoning = Some(ReasoningSettings::Anthropic {
            budget_tokens: 3000,
        });
        let picker = EffortPicker::new(&app);
        assert_eq!(picker.options[picker.selected], picker.active);
        for option in picker.options.iter().flatten() {
            let ReasoningSettings::Anthropic { budget_tokens } = option else {
                panic!("wrong protocol");
            };
            assert!((1024..10_000).contains(budget_tokens));
        }
        let candidate = picker.candidate(&app.settings).unwrap();
        let restored: Settings =
            toml::from_str(&toml::to_string_pretty(&candidate).unwrap()).unwrap();
        let model = restored.provider().settings_for_model(&restored.model);
        assert_eq!(model.parameters.reasoning, picker.active);
        assert_eq!(model.parameters.max_output_tokens, Some(10_000));
        assert!(model.capabilities.max_output_tokens.is_none());
    }

    #[tokio::test]
    async fn effort_rejects_stale_selections_and_allows_resetting_unsupported_models() {
        let mut app = app();
        let mut picker = EffortPicker::new(&app);
        picker.selected = 1;
        let model = app.settings.providers[0]
            .model_settings
            .entry(app.settings.model.clone())
            .or_default();
        model.capabilities.reasoning = CapabilitySupport::Unsupported;
        assert!(picker.candidate(&app.settings).is_err());
        let picker = EffortPicker::new(&app);
        assert!(picker.unsupported);
        assert_eq!(picker.options, [None]);
        assert!(picker.candidate(&app.settings).is_ok());
        app.settings.providers.clear();
        assert!(picker.candidate(&app.settings).is_err());
    }
}
