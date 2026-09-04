//! 极简 i18n：内置中英两套文案，全部为编译期常量。
//!
//! 语言跟随 `Settings.language`，设置中切换、Ctrl+S 保存后生效。
//! 静态文案是 [`Texts`] 的字段（少一个字段就编译不过），带参数的文案是方法。

use serde::{Deserialize, Serialize};

use crate::{
    app::{ContextField, NetworkField, SettingsField},
    config::{ApiKind, ProxyMode, ProxyValidationError},
    runtime::{
        conversation::MessageStatus,
        error::{RuntimeError, RuntimeErrorKind},
        model::StopReason,
    },
};

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    /// 设置界面里展示的语言名（各用本语言书写）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Zh => "中文",
            Self::En => "English",
        }
    }

    /// 设置界面里 Enter 直接在两种语言间切换。
    pub fn toggle(self) -> Self {
        match self {
            Self::Zh => Self::En,
            Self::En => Self::Zh,
        }
    }

    pub fn texts(self) -> Texts {
        match self {
            Self::Zh => Texts::ZH,
            Self::En => Texts::EN,
        }
    }

    /// 保存配置失败的提示。
    pub fn notice_save_failed(self, err: &str) -> String {
        match self {
            Self::Zh => format!("保存配置失败: {err}"),
            Self::En => format!("failed to save settings: {err}"),
        }
    }

    /// 会话创建、保存或删除失败的提示。
    pub fn notice_session_storage_failed(self, err: &str) -> String {
        match self {
            Self::Zh => format!("会话存储失败: {err}"),
            Self::En => format!("session storage failed: {err}"),
        }
    }

    pub const fn rename_empty_error(self) -> &'static str {
        match self {
            Self::Zh => "会话标题不能为空",
            Self::En => "Session title cannot be empty",
        }
    }

    pub const fn recovery_missing_model(self) -> &'static str {
        match self {
            Self::Zh => "该未完成回复没有模型快照，只能保留或丢弃。",
            Self::En => "This unfinished response has no model snapshot; keep or discard it.",
        }
    }

    pub fn recovery_model_unavailable(self, provider: &str, model: &str) -> String {
        match self {
            Self::Zh => format!("原模型 {provider} / {model} 已不可用；请保留或丢弃后重新发送。"),
            Self::En => format!(
                "The original model {provider} / {model} is unavailable; keep or discard, then resend."
            ),
        }
    }

    pub const fn recovery_continue_prompt(self) -> &'static str {
        match self {
            Self::Zh => "请从上一条因应用中断而未完成的回复末尾继续，不要重复已经给出的内容。",
            Self::En => {
                "Continue from the end of the previous response interrupted by the application restart. Do not repeat completed content."
            }
        }
    }

    pub fn notice_compacted(self, archived: usize, automatic: bool) -> String {
        match (self, automatic) {
            (Self::Zh, true) => {
                format!("已自动压缩上下文：归档 {archived} 条消息；可输入 /undo-compact 撤销")
            }
            (Self::Zh, false) => {
                format!("已压缩上下文：归档 {archived} 条消息；可输入 /undo-compact 撤销")
            }
            (Self::En, true) => format!(
                "Context auto-compacted: {archived} messages archived; use /undo-compact to undo"
            ),
            (Self::En, false) => format!(
                "Context compacted: {archived} messages archived; use /undo-compact to undo"
            ),
        }
    }

    pub const fn notice_nothing_to_compact(self) -> &'static str {
        match self {
            Self::Zh => "当前没有可压缩的旧消息",
            Self::En => "No old messages can be compacted",
        }
    }

    pub const fn notice_compaction_undone(self) -> &'static str {
        match self {
            Self::Zh => "已撤销最近一次上下文压缩并恢复原始历史",
            Self::En => "The latest compaction was undone and original history restored",
        }
    }

    pub const fn notice_nothing_to_undo(self) -> &'static str {
        match self {
            Self::Zh => "当前没有可撤销的上下文压缩",
            Self::En => "There is no context compaction to undo",
        }
    }

    /// Responses API 报告的生成失败。
    #[cfg(test)]
    pub fn llm_generation_failed(self, message: &str) -> String {
        match self {
            Self::Zh => format!("生成失败: {message}"),
            Self::En => format!("generation failed: {message}"),
        }
    }

    pub fn notice_no_api_key(self, kind: ApiKind) -> String {
        let envs = kind.api_key_envs().join(" / ");
        match self {
            Self::Zh => {
                format!("未配置 API Key：在供应商设置填写、配置鉴权 Header，或设置环境变量 {envs}")
            }
            Self::En => {
                format!("No API key: configure the Provider, add an auth header, or set {envs}")
            }
        }
    }

    pub fn runtime_error_kind_label(self, kind: RuntimeErrorKind) -> &'static str {
        match (self, kind) {
            (Self::Zh, RuntimeErrorKind::Authentication) => "鉴权错误",
            (Self::Zh, RuntimeErrorKind::RateLimit) => "限流错误",
            (Self::Zh, RuntimeErrorKind::Timeout) => "超时错误",
            (Self::Zh, RuntimeErrorKind::Provider) => "Provider 错误",
            (Self::Zh, RuntimeErrorKind::Protocol) => "协议错误",
            (Self::Zh, RuntimeErrorKind::ContextOverflow) => "上下文溢出",
            (Self::Zh, RuntimeErrorKind::Mcp) => "MCP 错误",
            (Self::Zh, RuntimeErrorKind::Configuration) => "配置错误",
            (Self::Zh, RuntimeErrorKind::Cancelled) => "任务已取消",
            (Self::Zh, RuntimeErrorKind::Unknown) => "未知错误",
            (Self::En, RuntimeErrorKind::Authentication) => "Authentication Error",
            (Self::En, RuntimeErrorKind::RateLimit) => "Rate Limit Error",
            (Self::En, RuntimeErrorKind::Timeout) => "Timeout Error",
            (Self::En, RuntimeErrorKind::Provider) => "Provider Error",
            (Self::En, RuntimeErrorKind::Protocol) => "Protocol Error",
            (Self::En, RuntimeErrorKind::ContextOverflow) => "Context Overflow",
            (Self::En, RuntimeErrorKind::Mcp) => "MCP Error",
            (Self::En, RuntimeErrorKind::Configuration) => "Configuration Error",
            (Self::En, RuntimeErrorKind::Cancelled) => "Task Cancelled",
            (Self::En, RuntimeErrorKind::Unknown) => "Unknown Error",
        }
    }

    pub fn runtime_error_notice(self, error: &RuntimeError) -> String {
        match self {
            Self::Zh => format!(
                "{}：{} · F2 查看脱敏详情",
                self.runtime_error_kind_label(error.kind),
                error.summary
            ),
            Self::En => format!(
                "{}: {} · F2 redacted details",
                self.runtime_error_kind_label(error.kind),
                error.summary
            ),
        }
    }

    pub fn runtime_error_recovery(self, kind: RuntimeErrorKind) -> &'static str {
        match (self, kind) {
            (Self::Zh, RuntimeErrorKind::Authentication) => {
                "检查 API Key、鉴权 Header、Provider 地址以及密钥权限后重试。"
            }
            (Self::Zh, RuntimeErrorKind::RateLimit) => {
                "等待 Retry-After 指示的时间，或降低请求频率、检查配额。"
            }
            (Self::Zh, RuntimeErrorKind::Timeout) => {
                "检查网络或代理，并按需调大首包、空闲或整体超时。"
            }
            (Self::Zh, RuntimeErrorKind::Protocol) => {
                "确认端点与所选协议一致；若是兼容服务，请携 request id 和详情排查。"
            }
            (Self::Zh, RuntimeErrorKind::ContextOverflow) => {
                "缩短会话内容、降低最大输出，或切换到上下文窗口更大的模型。"
            }
            (Self::Zh, RuntimeErrorKind::Configuration) => {
                "检查 Provider、模型参数、自定义 Header 和代理设置。"
            }
            (Self::Zh, RuntimeErrorKind::Mcp) => "检查 MCP Server 状态、配置和连接日志。",
            (Self::Zh, RuntimeErrorKind::Provider | RuntimeErrorKind::Unknown) => {
                "稍后重试；若持续失败，请用 request id 对照 Provider 日志。"
            }
            (Self::Zh, RuntimeErrorKind::Cancelled) => "任务已安全停止，可直接重新发送。",
            (Self::En, RuntimeErrorKind::Authentication) => {
                "Check the API key, auth headers, Provider URL, and key permissions."
            }
            (Self::En, RuntimeErrorKind::RateLimit) => {
                "Wait for Retry-After, reduce request frequency, or check quota."
            }
            (Self::En, RuntimeErrorKind::Timeout) => {
                "Check the network or proxy and adjust response/idle/overall timeouts."
            }
            (Self::En, RuntimeErrorKind::Protocol) => {
                "Verify that the endpoint matches the selected protocol; use the request id for diagnosis."
            }
            (Self::En, RuntimeErrorKind::ContextOverflow) => {
                "Shorten the conversation, lower max output, or choose a larger context window."
            }
            (Self::En, RuntimeErrorKind::Configuration) => {
                "Check Provider, model parameters, custom headers, and proxy settings."
            }
            (Self::En, RuntimeErrorKind::Mcp) => {
                "Check the MCP server status, configuration, and connection logs."
            }
            (Self::En, RuntimeErrorKind::Provider | RuntimeErrorKind::Unknown) => {
                "Retry later; if it persists, correlate the request id with Provider logs."
            }
            (Self::En, RuntimeErrorKind::Cancelled) => {
                "The task stopped safely and can be sent again."
            }
        }
    }

    pub const fn footer_task_hint(self) -> &'static str {
        match self {
            Self::Zh => "Esc/Ctrl+C 取消",
            Self::En => "Esc/Ctrl+C Cancel",
        }
    }

    pub fn message_status_label(self, status: MessageStatus) -> Option<&'static str> {
        match (self, status) {
            (_, MessageStatus::Completed) => None,
            (Self::Zh, MessageStatus::Streaming) => Some("生成中"),
            (Self::Zh, MessageStatus::Cancelled) => Some("已取消"),
            (Self::Zh, MessageStatus::Failed) => Some("失败"),
            (Self::En, MessageStatus::Streaming) => Some("Generating"),
            (Self::En, MessageStatus::Cancelled) => Some("Cancelled"),
            (Self::En, MessageStatus::Failed) => Some("Failed"),
        }
    }

    pub fn empty_assistant_placeholder(self, status: MessageStatus) -> &'static str {
        match (self, status) {
            (Self::Zh, MessageStatus::Streaming) => "（尚无回复正文）",
            (Self::Zh, MessageStatus::Cancelled) => "（生成在返回正文前被取消）",
            (Self::Zh, MessageStatus::Failed) => "（生成在返回正文前失败）",
            (Self::Zh, MessageStatus::Completed) => "（空回复）",
            (Self::En, MessageStatus::Streaming) => "(no response text yet)",
            (Self::En, MessageStatus::Cancelled) => "(cancelled before response text arrived)",
            (Self::En, MessageStatus::Failed) => "(failed before response text arrived)",
            (Self::En, MessageStatus::Completed) => "(empty response)",
        }
    }

    pub fn stop_reason_notice(self, reason: &StopReason) -> String {
        match (self, reason) {
            (Self::Zh, StopReason::MaxOutputTokens) => {
                "回复达到最大输出限制，内容可能被截断".to_owned()
            }
            (Self::Zh, StopReason::ContentFilter) => "回复被 Provider 的内容策略中止".to_owned(),
            (Self::Zh, StopReason::Refusal) => "模型拒绝了本次请求".to_owned(),
            (Self::Zh, StopReason::Other(reason)) => format!("Provider 以 {reason} 原因结束回复"),
            (Self::En, StopReason::MaxOutputTokens) => {
                "Response reached its output limit and may be truncated".to_owned()
            }
            (Self::En, StopReason::ContentFilter) => {
                "The Provider stopped the response for content policy reasons".to_owned()
            }
            (Self::En, StopReason::Refusal) => "The model refused this request".to_owned(),
            (Self::En, StopReason::Other(reason)) => {
                format!("Provider stopped with reason {reason}")
            }
            (_, StopReason::Stop | StopReason::StopSequence) => String::new(),
        }
    }

    /// 模型同步成功的提示。
    pub fn wizard_sync_ok(self, count: usize) -> String {
        match self {
            Self::Zh => format!("已获取 {count} 个模型"),
            Self::En => format!("{count} models found"),
        }
    }

    /// 设置页中的代理校验错误。
    pub fn proxy_validation_error(self, error: ProxyValidationError) -> String {
        match (self, error) {
            (Self::Zh, ProxyValidationError::MissingUrl) => "请填写代理 URL".to_owned(),
            (Self::Zh, ProxyValidationError::InvalidUrl) => {
                "代理 URL 无效，请包含完整协议和地址".to_owned()
            }
            (Self::Zh, ProxyValidationError::MissingHost) => {
                "代理 URL 必须包含主机名或 IP".to_owned()
            }
            (Self::Zh, ProxyValidationError::SchemeMismatch) => {
                "代理 URL 协议与所选类型不匹配".to_owned()
            }
            (Self::Zh, ProxyValidationError::QueryOrFragment) => {
                "代理 URL 不能包含查询参数或片段".to_owned()
            }
            (Self::Zh, ProxyValidationError::EnvironmentUnavailable) => {
                "代理引用的环境变量不存在或不是有效文本".to_owned()
            }
            (Self::En, ProxyValidationError::MissingUrl) => "Enter a proxy URL".to_owned(),
            (Self::En, ProxyValidationError::InvalidUrl) => {
                "Invalid proxy URL; include its scheme and address".to_owned()
            }
            (Self::En, ProxyValidationError::MissingHost) => {
                "Proxy URL must include a hostname or IP".to_owned()
            }
            (Self::En, ProxyValidationError::SchemeMismatch) => {
                "Proxy URL scheme does not match the selected type".to_owned()
            }
            (Self::En, ProxyValidationError::QueryOrFragment) => {
                "Proxy URL cannot contain a query or fragment".to_owned()
            }
            (Self::En, ProxyValidationError::EnvironmentUnavailable) => {
                "The environment variable referenced by the proxy is unavailable".to_owned()
            }
        }
    }
}

/// 一套界面文案。
#[derive(Debug, Clone, Copy)]
pub struct Texts {
    // Header / Sidebar
    pub header_chat: &'static str,
    pub sidebar_history: &'static str,
    pub sidebar_hint: &'static str,
    pub sidebar_settings: &'static str,
    pub sidebar_settings_hint: &'static str,
    // Chat
    pub role_user: &'static str,
    pub chat_input: &'static str,
    // 模型切换弹窗
    pub picker_session_title: &'static str,
    pub picker_session_hint: &'static str,
    pub picker_turn_title: &'static str,
    pub picker_turn_hint: &'static str,
    pub picker_empty: &'static str,
    pub picker_new_label: &'static str,
    // Footer
    pub footer_ctx: &'static str,
    pub footer_speed: &'static str,
    pub footer_generating: &'static str,
    pub footer_aborted: &'static str,
    pub footer_quit_hint: &'static str,
    // 设置：框架与分类
    pub settings_title: &'static str,
    pub settings_bottom_hint: &'static str,
    pub cat_models: &'static str,
    pub cat_context: &'static str,
    pub cat_mcp: &'static str,
    pub cat_network: &'static str,
    pub cat_keyboard: &'static str,
    pub cat_appearance: &'static str,
    // 设置：模型分类
    pub providers_title: &'static str,
    pub providers_hint: &'static str,
    pub models_list_title: &'static str,
    pub models_hint: &'static str,
    pub syncing: &'static str,
    pub manual_add_label: &'static str,
    // 设置：网络分类
    pub field_proxy_mode: &'static str,
    pub field_proxy_url: &'static str,
    pub proxy_disabled: &'static str,
    pub proxy_url_hint: &'static str,
    pub proxy_edit_hint: &'static str,
    // 设置：上下文分类
    pub field_auto_compact: &'static str,
    pub field_compact_threshold: &'static str,
    pub field_reserved_output: &'static str,
    pub context_settings_hint: &'static str,
    // 设置：外观分类
    pub field_language: &'static str,
    pub field_theme: &'static str,
    pub field_titan_art: &'static str,
    pub value_on: &'static str,
    pub value_off: &'static str,
    pub settings_toggle_hint: &'static str,
    // 新增供应商向导
    pub wizard_step1_title: &'static str,
    pub wizard_step2_title: &'static str,
    pub wizard_step3_title: &'static str,
    pub wizard_edit_step1_title: &'static str,
    pub wizard_edit_step2_title: &'static str,
    pub wizard_edit_step3_title: &'static str,
    pub wizard_kind_chat_hint: &'static str,
    pub wizard_kind_resp_hint: &'static str,
    pub wizard_kind_anthropic_hint: &'static str,
    pub wizard_kind_gemini_hint: &'static str,
    pub wizard_name: &'static str,
    pub wizard_name_hint: &'static str,
    pub wizard_url: &'static str,
    pub wizard_key: &'static str,
    pub wizard_headers: &'static str,
    pub wizard_url_hint: &'static str,
    pub wizard_step2_hint: &'static str,
    pub wizard_step3_hint: &'static str,
    pub wizard_need_one: &'static str,
    pub wizard_retry_hint: &'static str,
    pub wizard_step1_hint: &'static str,
    // 临时提示
    pub notice_generating: &'static str,
    pub notice_aborted: &'static str,
    // 会话
    pub new_session_title: &'static str,
    // LLM 标题生成
    pub title_system_prompt: &'static str,
    pub llm_empty: &'static str,
}

impl Texts {
    pub const ZH: Self = Self {
        header_chat: "会话",
        sidebar_history: "历史会话",
        sidebar_hint: "n 新建 · r 重命名 · / 搜索 · d 删除",
        sidebar_settings: "⚙ 设置",
        sidebar_settings_hint: "s 打开设置",
        role_user: "你",
        chat_input: "输入",
        picker_session_title: "会话默认模型",
        picker_session_hint: "Enter 应用 · ↑↓ 选择 · Tab 跳转供应商 · a/d 管理 · Esc",
        picker_turn_title: "下一轮临时模型",
        picker_turn_hint: "Enter 下一轮 · ↑↓ 选择 · Tab 跳转供应商 · a/d 管理 · Esc",
        picker_empty: "(列表为空，按 a 添加)",
        picker_new_label: "新模型名",
        footer_ctx: "上下文",
        footer_speed: "速度",
        footer_generating: "生成中",
        footer_aborted: "已取消",
        footer_quit_hint: "Ctrl+C 退出",
        settings_title: "设置",
        settings_bottom_hint: "↑↓ 移动 · Tab 切换栏 · Ctrl+S 保存 · Esc 返回",
        cat_models: "模型",
        cat_context: "上下文",
        cat_mcp: "MCP",
        cat_network: "网络",
        cat_keyboard: "键盘",
        cat_appearance: "外观",
        providers_title: "供应商",
        providers_hint: "a 新增 · e 编辑 · d 删除 · Enter 当前",
        models_list_title: "模型",
        models_hint: "Enter 使用 · s 同步 · n 手动添加",
        syncing: "同步中",
        manual_add_label: "手动添加",
        field_proxy_mode: "代理类型",
        field_proxy_url: "代理 URL",
        proxy_disabled: "不使用",
        proxy_url_hint: "HTTP: http(s)://host:port · SOCKS5: socks5(h)://host:port",
        proxy_edit_hint: "Enter 编辑/确认 · Esc 取消；URL 可包含用户名与密码",
        field_auto_compact: "自动压缩",
        field_compact_threshold: "触发阈值",
        field_reserved_output: "预留输出",
        context_settings_hint: "←/→ 调整 · Enter 切换/增加；token 为无 tokenizer 时的保守预算",
        field_language: "语言",
        field_theme: "主题",
        field_titan_art: "泰坦背景",
        value_on: "开",
        value_off: "关",
        settings_toggle_hint: "Enter 切换",
        wizard_step1_title: "新增供应商 1/3 · 选择协议",
        wizard_step2_title: "新增供应商 2/3 · 连接配置",
        wizard_step3_title: "新增供应商 3/3 · 选择模型",
        wizard_edit_step1_title: "编辑供应商 1/3 · 选择协议",
        wizard_edit_step2_title: "编辑供应商 2/3 · 连接配置",
        wizard_edit_step3_title: "编辑供应商 3/3 · 更新模型",
        wizard_kind_chat_hint: "兼容性最好，多数服务支持",
        wizard_kind_resp_hint: "OpenAI 新接口",
        wizard_kind_anthropic_hint: "Anthropic 原生 Messages API",
        wizard_kind_gemini_hint: "Google Gemini 原生 GenerateContent API",
        wizard_name: "名称",
        wizard_name_hint: "(留空自动取域名)",
        wizard_url: "Base URL",
        wizard_key: "API Key",
        wizard_headers: "Headers JSON",
        wizard_url_hint: "URL 留空使用所选协议官方地址；Key 留空读取对应环境变量；Headers 使用 JSON object",
        wizard_step2_hint: "Tab/↑/↓ 切换字段 · Enter 同步模型 · Esc 上一步",
        wizard_step3_hint: "空格 勾选 · a 全选/全不选 · Enter 保存",
        wizard_need_one: "至少选择 1 个模型",
        wizard_retry_hint: "r 重试 · 也可在下方手动添加",
        wizard_step1_hint: "↑↓ 选择 · Enter 下一步 · Esc 取消",
        notice_generating: "生成中，请稍候或按 Esc/Ctrl+C 取消",
        notice_aborted: "已取消生成",
        new_session_title: "新会话",
        title_system_prompt: "为下面的对话生成一个简短标题，不超过 10 个字。\
只输出标题本身，不要解释、引号或标点。",
        llm_empty: "（空）",
    };

    pub const EN: Self = Self {
        header_chat: "Chat",
        sidebar_history: "History",
        sidebar_hint: "n New · r Rename · / Search · d Del",
        sidebar_settings: "⚙ Settings",
        sidebar_settings_hint: "press s",
        role_user: "You",
        chat_input: "Input",
        picker_session_title: "Session Default Model",
        picker_session_hint: "Enter Apply · ↑↓ Select · Tab Provider · a/d Manage · Esc",
        picker_turn_title: "Next-turn Model",
        picker_turn_hint: "Enter Next Turn · ↑↓ Select · Tab Provider · a/d Manage · Esc",
        picker_empty: "(empty — press a to add)",
        picker_new_label: "New model",
        footer_ctx: "Context",
        footer_speed: "Speed",
        footer_generating: "Generating",
        footer_aborted: "Cancelled",
        footer_quit_hint: "Ctrl+C Quit",
        settings_title: "Settings",
        settings_bottom_hint: "↑↓ Move · Tab Pane · Ctrl+S Save · Esc Back",
        cat_models: "Models",
        cat_context: "Context",
        cat_mcp: "MCP",
        cat_network: "Network",
        cat_keyboard: "Keyboard",
        cat_appearance: "Appearance",
        providers_title: "Providers",
        providers_hint: "a Add · e Edit · d Delete · Enter Use",
        models_list_title: "Models",
        models_hint: "Enter Use · s Sync · n Add",
        syncing: "Syncing",
        manual_add_label: "Add model",
        field_proxy_mode: "Proxy type",
        field_proxy_url: "Proxy URL",
        proxy_disabled: "Direct",
        proxy_url_hint: "HTTP: http(s)://host:port · SOCKS5: socks5(h)://host:port",
        proxy_edit_hint: "Enter edit/confirm · Esc cancel; credentials may be embedded",
        field_auto_compact: "Auto compact",
        field_compact_threshold: "Threshold",
        field_reserved_output: "Output reserve",
        context_settings_hint: "←/→ adjust · Enter toggle/increase; token counts are conservative without a tokenizer",
        field_language: "Language",
        field_theme: "Theme",
        field_titan_art: "Titan Art",
        value_on: "on",
        value_off: "off",
        settings_toggle_hint: "Enter to toggle",
        wizard_step1_title: "Add Provider 1/3 · Protocol",
        wizard_step2_title: "Add Provider 2/3 · Connection",
        wizard_step3_title: "Add Provider 3/3 · Pick Models",
        wizard_edit_step1_title: "Edit Provider 1/3 · Protocol",
        wizard_edit_step2_title: "Edit Provider 2/3 · Connection",
        wizard_edit_step3_title: "Edit Provider 3/3 · Update Models",
        wizard_kind_chat_hint: "best compatibility",
        wizard_kind_resp_hint: "new OpenAI API",
        wizard_kind_anthropic_hint: "native Anthropic Messages API",
        wizard_kind_gemini_hint: "native Google Gemini GenerateContent API",
        wizard_name: "Name",
        wizard_name_hint: "(blank = hostname)",
        wizard_url: "Base URL",
        wizard_key: "API Key",
        wizard_headers: "Headers JSON",
        wizard_url_hint: "blank URL = selected official endpoint; blank key = protocol env; Headers = JSON object",
        wizard_step2_hint: "Tab/↑/↓ Next Field · Enter Sync · Esc Back",
        wizard_step3_hint: "Space Toggle · a All/None · Enter Save",
        wizard_need_one: "select at least 1 model",
        wizard_retry_hint: "r Retry · or add manually below",
        wizard_step1_hint: "↑↓ Select · Enter Next · Esc Cancel",
        notice_generating: "Generating — wait or press Esc/Ctrl+C to cancel",
        notice_aborted: "Generation cancelled",
        new_session_title: "New chat",
        title_system_prompt: "Generate a short title for the conversation below, at most 6 words. \
Output only the title itself: no explanations, quotes, or punctuation.",
        llm_empty: "(empty)",
    };

    /// 设置弹窗「外观」分类的字段标签。
    pub fn field_label(&self, field: SettingsField) -> &'static str {
        match field {
            SettingsField::Language => self.field_language,
            SettingsField::Theme => self.field_theme,
            SettingsField::TitanArt => self.field_titan_art,
        }
    }

    pub fn network_field_label(&self, field: NetworkField) -> &'static str {
        match field {
            NetworkField::ProxyMode => self.field_proxy_mode,
            NetworkField::ProxyUrl => self.field_proxy_url,
        }
    }

    pub fn context_field_label(&self, field: ContextField) -> &'static str {
        match field {
            ContextField::AutoCompact => self.field_auto_compact,
            ContextField::Threshold => self.field_compact_threshold,
            ContextField::ReservedOutput => self.field_reserved_output,
        }
    }

    pub fn proxy_mode_label(&self, mode: ProxyMode) -> &'static str {
        match mode {
            ProxyMode::Disabled => self.proxy_disabled,
            ProxyMode::Http => "HTTP",
            ProxyMode::Socks5 => "SOCKS5",
        }
    }

    pub fn wizard_title(&self, editing: bool, step: u8) -> &'static str {
        match (editing, step) {
            (false, 1) => self.wizard_step1_title,
            (false, 2) => self.wizard_step2_title,
            (false, _) => self.wizard_step3_title,
            (true, 1) => self.wizard_edit_step1_title,
            (true, 2) => self.wizard_edit_step2_title,
            (true, _) => self.wizard_edit_step3_title,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_available() {
        assert_eq!(Lang::Zh.label(), "中文");
        assert_eq!(Lang::En.label(), "English");
        assert_eq!(Lang::Zh.toggle(), Lang::En);
        assert_eq!(Lang::En.toggle(), Lang::Zh);
    }

    #[test]
    fn lang_serde_names() {
        // TOML 顶层不能是裸值，经由 Settings 验证字段名
        let zh: crate::config::Settings = toml::from_str("language = \"zh\"").unwrap();
        let en: crate::config::Settings = toml::from_str("language = \"en\"").unwrap();
        assert_eq!(zh.language, Lang::Zh);
        assert_eq!(en.language, Lang::En);
    }

    #[test]
    fn texts_differ_between_languages() {
        assert_ne!(Texts::ZH.sidebar_history, Texts::EN.sidebar_history);
        assert_ne!(Texts::ZH.new_session_title, Texts::EN.new_session_title);
        assert_ne!(Texts::ZH.providers_hint, Texts::EN.providers_hint);
    }

    #[test]
    fn field_labels_cover_appearance_fields() {
        for lang in [Lang::Zh, Lang::En] {
            let texts = lang.texts();
            for field in SettingsField::ALL {
                assert!(!texts.field_label(field).is_empty());
            }
            for field in NetworkField::ALL {
                assert!(!texts.network_field_label(field).is_empty());
            }
        }
    }

    #[test]
    fn dynamic_notices() {
        assert_eq!(Lang::Zh.notice_save_failed("io"), "保存配置失败: io");
        assert_eq!(
            Lang::En.notice_session_storage_failed("io"),
            "session storage failed: io"
        );
        assert_eq!(
            Lang::En.llm_generation_failed("401"),
            "generation failed: 401"
        );
        assert_eq!(Lang::Zh.wizard_sync_ok(12), "已获取 12 个模型");
        assert_eq!(
            Lang::Zh.proxy_validation_error(ProxyValidationError::SchemeMismatch),
            "代理 URL 协议与所选类型不匹配"
        );
        let error = RuntimeError {
            kind: RuntimeErrorKind::RateLimit,
            summary: "HTTP 429".to_owned(),
            detail: "HTTP 429".to_owned(),
            request_id: None,
            retry_after_ms: Some(1000),
        };
        assert!(Lang::Zh.runtime_error_notice(&error).contains("限流错误"));
        assert!(
            Lang::En
                .runtime_error_recovery(error.kind)
                .contains("Retry-After")
        );
    }
}
