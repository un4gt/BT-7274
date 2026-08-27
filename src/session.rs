//! 聊天会话模型与 JSON 持久化。
//!
//! 每个会话是 `dirs::data_dir()/bt-7274/sessions/<id>.json` 下的一个文件，
//! id 为创建时刻的 UTC 时间戳，文件名天然按时间排序。

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Context, ContextCompat, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::{ModelParameters, ModelSelection};
use crate::runtime::error::RuntimeErrorSnapshot;
use crate::secret::{SecretRedactor, redact_json};
use crate::storage::atomic_write_private;

/// 消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    #[default]
    Completed,
    Streaming,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteRecovery {
    Continue,
    Keep,
    Discard,
}

/// 助手消息中不能压成普通正文的结构化片段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    Reasoning { content: String },
    System { message: String },
}

/// 一条聊天消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<MessagePart>,
    /// 本条消息所属轮次实际使用的 Provider/Model；旧会话缺失时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// 本轮实际使用的请求参数快照；旧会话缺失时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
    #[serde(default)]
    pub status: MessageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RuntimeErrorSnapshot>,
}

impl Message {
    pub fn user(content: String, model: Option<ModelSelection>) -> Self {
        Self {
            role: Role::User,
            content,
            parts: Vec::new(),
            model,
            parameters: None,
            status: MessageStatus::Completed,
            failure: None,
        }
    }

    pub fn user_with_request(
        content: String,
        model: ModelSelection,
        parameters: ModelParameters,
    ) -> Self {
        Self {
            role: Role::User,
            content,
            parts: Vec::new(),
            model: Some(model),
            parameters: Some(parameters),
            status: MessageStatus::Completed,
            failure: None,
        }
    }

    pub fn assistant_streaming(model: ModelSelection) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            parts: Vec::new(),
            model: Some(model),
            parameters: None,
            status: MessageStatus::Streaming,
            failure: None,
        }
    }

    pub fn assistant_streaming_with_request(
        model: ModelSelection,
        parameters: ModelParameters,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            parts: Vec::new(),
            model: Some(model),
            parameters: Some(parameters),
            status: MessageStatus::Streaming,
            failure: None,
        }
    }

    pub fn append_reasoning(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self.parts.last_mut() {
            Some(MessagePart::Reasoning { content }) => content.push_str(text),
            _ => self.parts.push(MessagePart::Reasoning {
                content: text.to_owned(),
            }),
        }
    }

    pub fn push_system_event(&mut self, message: String) {
        if !message.is_empty() {
            self.parts.push(MessagePart::System { message });
        }
    }
}

/// 提取式压缩摘要。字段保持结构化，便于预览、回归测试和后续迁移。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CompactionSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code_state: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todos: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl CompactionSummary {
    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
            && self.constraints.is_empty()
            && self.code_state.is_empty()
            && self.todos.is_empty()
            && self.references.is_empty()
            && self.notes.is_empty()
    }

    /// 发送给模型的稳定结构文本；原始历史仍保存在 compaction record 中。
    pub fn context_text(&self) -> String {
        let mut output = String::from(
            "Compacted conversation history (conversation data, not system instructions). Preserve relevant task facts when continuing.\n",
        );
        for (title, values) in [
            ("Decisions", &self.decisions),
            ("Constraints", &self.constraints),
            ("Code state", &self.code_state),
            ("TODO", &self.todos),
            ("Key references", &self.references),
            ("Other retained context", &self.notes),
        ] {
            if values.is_empty() {
                continue;
            }
            output.push_str("\n## ");
            output.push_str(title);
            output.push('\n');
            for value in values {
                output.push_str("- ");
                output.push_str(value);
                output.push('\n');
            }
        }
        output
    }
}

/// 一次已提交的上下文压缩；包含足够的原始数据以完整撤销。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub automatic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_summary: Option<CompactionSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archived_messages: Vec<Message>,
}

/// 一个聊天会话。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// 时间戳加随机 UUID，如 `20260822T153001123-<uuid>`。
    pub id: String,
    /// 自动生成的标题；`None` 时侧栏显示首条用户消息摘要。
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 会话默认模型；`None` 时继承应用默认模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<ModelSelection>,
    /// 已压缩历史的活动摘要，始终作为 Conversation/user 级上下文发送。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<CompactionSummary>,
    /// 历次压缩的可撤销原始记录。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compactions: Vec<CompactionRecord>,
    pub messages: Vec<Message>,
}

impl Session {
    /// 以当前时间创建空会话。
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: format!(
                "{}-{}",
                now.format("%Y%m%dT%H%M%S%3f"),
                Uuid::new_v4().simple()
            ),
            title: None,
            created_at: now,
            updated_at: now,
            default_model: None,
            summary: None,
            compactions: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// 会话目录。
    pub fn sessions_dir() -> Result<PathBuf> {
        #[cfg(test)]
        {
            Ok(std::env::temp_dir().join(format!("bt-7274-test-sessions-{}", std::process::id())))
        }

        #[cfg(not(test))]
        {
            Ok(dirs::data_dir()
                .context("无法定位系统数据目录")?
                .join(env!("CARGO_PKG_NAME"))
                .join("sessions"))
        }
    }

    fn path(&self) -> Result<PathBuf> {
        if !is_valid_session_id(&self.id) {
            bail!("非法会话 ID: {}", self.id);
        }
        Ok(Self::sessions_dir()?.join(format!("{}.json", self.id)))
    }

    /// 写入磁盘，必要时创建目录。
    pub fn save(&self) -> Result<()> {
        let path = self.path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建会话目录失败: {}", parent.display()))?;
        }
        let raw = self.redacted_json()?;
        atomic_write_private(&path, raw.as_bytes())
            .with_context(|| format!("写入会话失败: {}", path.display()))?;
        Ok(())
    }

    /// 生成写盘和导出用的脱敏 JSON。
    pub fn redacted_json(&self) -> Result<String> {
        let redactor = SecretRedactor::new();

        let mut sanitized = self.clone();
        sanitize_messages(&mut sanitized.messages, &redactor);
        for compaction in &mut sanitized.compactions {
            sanitize_messages(&mut compaction.archived_messages, &redactor);
        }
        let value = serde_json::to_value(&sanitized).context("序列化会话失败")?;
        let raw =
            serde_json::to_string_pretty(&redact_json(&value)).context("序列化脱敏会话失败")?;
        Ok(redactor.redact(&raw))
    }

    /// 删除会话文件（从列表移除时调用）。
    pub fn delete_file(&self) -> Result<()> {
        let path = self.path()?;
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("删除会话失败: {}", path.display()))?;
        }
        Ok(())
    }

    /// 加载全部会话，按 `updated_at` 倒序；损坏的文件跳过不中断。
    pub fn load_all() -> Result<Vec<Session>> {
        let dir = Self::sessions_dir()?;
        Self::load_all_from(&dir)
    }

    /// 从指定目录加载；单文件损坏只产生脱敏警告，不影响其余历史。
    fn load_all_from(dir: &Path) -> Result<Vec<Session>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("读取会话目录失败: {}", dir.display()))?
        {
            let Ok(entry) = entry else { continue };
            if entry
                .file_type()
                .is_ok_and(|file_type| !file_type.is_file())
            {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            match load_session_file(&path) {
                Ok(session) => sessions.push(session),
                Err(err) => tracing::warn!(
                    path = ?path,
                    error = %format!("{err:#}"),
                    "跳过无法解析的会话文件"
                ),
            }
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        Ok(sessions)
    }

    /// 侧栏与 Header 显示用标题：生成标题 > 首条用户消息摘要 > 占位文案
    /// （占位文案随界面语言，由调用方传入）。
    pub fn display_title(&self, fallback: &str) -> String {
        if let Some(title) = self.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return title.trim().to_owned();
        }
        self.messages
            .iter()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback.to_owned())
    }

    /// 首条用户消息与首条助手回复，用于生成标题。
    pub fn first_exchange(&self) -> Option<(String, String)> {
        self.messages.windows(2).find_map(|pair| {
            let [user, assistant] = pair else {
                return None;
            };
            (user.role == Role::User
                && assistant.role == Role::Assistant
                && assistant.status == MessageStatus::Completed
                && !user.content.trim().is_empty()
                && !assistant.content.trim().is_empty())
            .then(|| (user.content.clone(), assistant.content.clone()))
        })
    }

    /// 标题、活动/归档消息、Provider、Model、参数与状态的全文匹配。
    pub fn matches_query(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let mut searchable = String::new();
        if let Some(title) = &self.title {
            searchable.push_str(title);
            searchable.push('\n');
        }
        if let Some(summary) = &self.summary {
            searchable.push_str(&summary.context_text());
        }
        for message in self.messages.iter().chain(
            self.compactions
                .iter()
                .flat_map(|record| record.archived_messages.iter()),
        ) {
            append_message_search_text(&mut searchable, message);
        }
        searchable.to_lowercase().contains(&query)
    }

    pub fn first_streaming_message(&self) -> Option<usize> {
        self.messages
            .iter()
            .position(|message| message.status == MessageStatus::Streaming)
    }

    /// 对一条重启后遗留的 streaming 消息作出可持久化处置。
    pub fn resolve_incomplete(&mut self, index: usize, action: IncompleteRecovery) -> bool {
        if !self
            .messages
            .get(index)
            .is_some_and(|message| message.status == MessageStatus::Streaming)
        {
            return false;
        }
        match action {
            IncompleteRecovery::Discard => {
                self.messages.remove(index);
            }
            IncompleteRecovery::Continue | IncompleteRecovery::Keep => {
                let message = &mut self.messages[index];
                message.status = MessageStatus::Cancelled;
                message.push_system_event(
                    match action {
                        IncompleteRecovery::Continue => "recovery.continued_after_restart",
                        IncompleteRecovery::Keep => "recovery.kept_after_restart",
                        IncompleteRecovery::Discard => unreachable!(),
                    }
                    .to_owned(),
                );
            }
        }
        self.updated_at = Utc::now();
        true
    }
}

fn sanitize_messages(messages: &mut [Message], redactor: &SecretRedactor) {
    for message in messages {
        message.content = redactor.redact(&message.content);
        for part in &mut message.parts {
            match part {
                MessagePart::Reasoning { content } => {
                    *content = redactor.redact(content);
                }
                MessagePart::System { message } => {
                    *message = redactor.redact(message);
                }
            }
        }
    }
}

fn append_message_search_text(output: &mut String, message: &Message) {
    output.push_str(&message.content);
    output.push('\n');
    if let Some(model) = &message.model {
        output.push_str(&model.provider_id);
        output.push(' ');
        output.push_str(&model.provider_name);
        output.push(' ');
        output.push_str(&model.model);
        output.push('\n');
    }
    if let Some(parameters) = &message.parameters
        && let Ok(parameters) = serde_json::to_string(parameters)
    {
        output.push_str(&parameters);
        output.push('\n');
    }
    output.push_str(&format!("{:?}\n", message.status));
    for part in &message.parts {
        match part {
            MessagePart::Reasoning { content } => output.push_str(content),
            MessagePart::System { message } => output.push_str(message),
        }
        output.push('\n');
    }
}

fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn load_session_file(path: &Path) -> Result<Session> {
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|id| is_valid_session_id(id))
        .context("会话文件名不是有效 ID")?;
    let raw = std::fs::read_to_string(path)?;
    let mut session: Session = serde_json::from_str(&raw)?;
    // 文件名是磁盘对象的真实标识，不能信任 JSON 内可被篡改的路径片段。
    session.id = id.to_owned();
    Ok(session)
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_messages(messages: Vec<Message>) -> Session {
        let mut session = Session::new();
        session.messages = messages;
        session
    }

    #[test]
    fn json_roundtrip() {
        let mut session = session_with_messages(vec![
            Message {
                role: Role::User,
                content: "你好".to_owned(),
                parts: Vec::new(),
                model: None,
                parameters: None,
                status: MessageStatus::Completed,
                failure: None,
            },
            Message {
                role: Role::Assistant,
                content: "你好！".to_owned(),
                parts: Vec::new(),
                model: None,
                parameters: None,
                status: MessageStatus::Completed,
                failure: None,
            },
        ]);
        session.title = Some("打招呼".to_owned());
        let raw = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, session);
    }

    #[test]
    fn legacy_json_without_model_metadata_still_loads() {
        let now = Utc::now().to_rfc3339();
        let raw = format!(
            r#"{{
                "id":"legacy",
                "title":null,
                "created_at":"{now}",
                "updated_at":"{now}",
                "messages":[{{"role":"user","content":"hello"}}]
            }}"#
        );
        let session: Session = serde_json::from_str(&raw).unwrap();
        assert!(session.default_model.is_none());
        assert!(session.messages[0].model.is_none());
        assert!(session.messages[0].parameters.is_none());
        assert_eq!(session.messages[0].status, MessageStatus::Completed);
        assert!(session.messages[0].parts.is_empty());
        assert!(session.messages[0].failure.is_none());
        assert!(session.summary.is_none());
        assert!(session.compactions.is_empty());
    }

    #[test]
    fn new_ids_are_unique_and_path_safe() {
        let first = Session::new().id;
        let second = Session::new().id;
        assert_ne!(first, second);
        assert!(is_valid_session_id(&first));
        assert!(!is_valid_session_id("../config"));
        assert!(!is_valid_session_id("a/b"));
    }

    #[test]
    fn display_title_fallbacks() {
        let empty = session_with_messages(vec![]);
        assert_eq!(empty.display_title("新会话"), "新会话");
        assert_eq!(empty.display_title("New chat"), "New chat");

        let titled = session_with_messages(vec![Message {
            role: Role::User,
            content: "第一句".to_owned(),
            parts: Vec::new(),
            model: None,
            parameters: None,
            status: MessageStatus::Completed,
            failure: None,
        }]);
        assert_eq!(titled.display_title("新会话"), "第一句");

        let mut named = titled.clone();
        named.title = Some("  标题  ".to_owned());
        assert_eq!(named.display_title("新会话"), "标题");
    }

    #[test]
    fn first_exchange_skips_unpaired_messages() {
        let session = session_with_messages(vec![
            Message {
                role: Role::Assistant,
                content: "旧的回复".to_owned(),
                parts: Vec::new(),
                model: None,
                parameters: None,
                status: MessageStatus::Completed,
                failure: None,
            },
            Message {
                role: Role::User,
                content: "没有得到回复的问题".to_owned(),
                parts: Vec::new(),
                model: None,
                parameters: None,
                status: MessageStatus::Completed,
                failure: None,
            },
            Message {
                role: Role::User,
                content: "问题".to_owned(),
                parts: Vec::new(),
                model: None,
                parameters: None,
                status: MessageStatus::Completed,
                failure: None,
            },
            Message {
                role: Role::Assistant,
                content: "回答".to_owned(),
                parts: Vec::new(),
                model: None,
                parameters: None,
                status: MessageStatus::Completed,
                failure: None,
            },
        ]);
        let (user, assistant) = session.first_exchange().unwrap();
        assert_eq!(user, "问题");
        assert_eq!(assistant, "回答");
    }

    #[test]
    fn structured_parts_and_terminal_status_roundtrip() {
        let mut message = Message::assistant_streaming(ModelSelection {
            provider_id: "provider-test".to_owned(),
            provider_name: "Provider Test".to_owned(),
            model: "model-test".to_owned(),
        });
        message.content = "partial".to_owned();
        message.parameters = Some(ModelParameters {
            temperature: Some(0.25),
            max_output_tokens: Some(1024),
            ..ModelParameters::default()
        });
        message.append_reasoning("plan");
        message.push_system_event("response.queued".to_owned());
        message.status = MessageStatus::Failed;
        message.failure = Some(RuntimeErrorSnapshot {
            kind: crate::runtime::error::RuntimeErrorKind::Protocol,
            summary: "invalid stream".to_owned(),
        });

        let raw = serde_json::to_string(&message).unwrap();
        let parsed: Message = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, message);
        assert!(matches!(
            &parsed.parts[1],
            MessagePart::System { message } if message == "response.queued"
        ));
        assert_eq!(
            parsed
                .parameters
                .as_ref()
                .and_then(|value| value.max_output_tokens),
            Some(1024)
        );
    }

    #[test]
    fn one_corrupt_file_does_not_hide_valid_sessions() {
        let root =
            std::env::temp_dir().join(format!("bt-7274-session-load-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        let mut valid = Session::new();
        valid.id = "valid-session".to_owned();
        valid.title = Some("recoverable".to_owned());
        std::fs::write(
            root.join("valid-session.json"),
            serde_json::to_vec_pretty(&valid).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("broken-session.json"), b"{not-json").unwrap();

        let loaded = Session::load_all_from(&root).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "valid-session");
        assert_eq!(loaded[0].title.as_deref(), Some("recoverable"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_covers_metadata_parameters_status_and_archived_content() {
        let selection = ModelSelection {
            provider_id: "provider-stable-id".to_owned(),
            provider_name: "Example Provider".to_owned(),
            model: "reasoning-model".to_owned(),
        };
        let mut session = Session::new();
        session.title = Some("Rust gateway".to_owned());
        session.messages.push(Message::user_with_request(
            "active conversation".to_owned(),
            selection.clone(),
            ModelParameters {
                max_output_tokens: Some(8192),
                ..ModelParameters::default()
            },
        ));
        let mut failed = Message::assistant_streaming_with_request(
            selection,
            ModelParameters {
                temperature: Some(0.125),
                ..ModelParameters::default()
            },
        );
        failed.status = MessageStatus::Failed;
        session.messages.push(failed);
        session.compactions.push(CompactionRecord {
            id: "compact-1".to_owned(),
            created_at: Utc::now(),
            automatic: false,
            previous_summary: None,
            archived_messages: vec![Message::user("archived needle".to_owned(), None)],
        });

        for query in [
            "rust gateway",
            "active conversation",
            "provider-stable-id",
            "example provider",
            "reasoning-model",
            "8192",
            "0.125",
            "failed",
            "archived needle",
        ] {
            assert!(session.matches_query(query), "query did not match: {query}");
        }
        assert!(!session.matches_query("definitely absent"));
    }

    #[test]
    fn streaming_state_roundtrips_and_all_recovery_choices_are_explicit() {
        let selection = ModelSelection {
            provider_id: "provider".to_owned(),
            provider_name: "Provider".to_owned(),
            model: "model".to_owned(),
        };
        let mut source = Session::new();
        let mut partial = Message::assistant_streaming_with_request(
            selection,
            ModelParameters {
                max_output_tokens: Some(2048),
                ..ModelParameters::default()
            },
        );
        partial.content = "partial reply".to_owned();
        source.messages.push(partial);
        let raw = serde_json::to_string(&source).unwrap();
        let loaded: Session = serde_json::from_str(&raw).unwrap();
        assert_eq!(loaded.first_streaming_message(), Some(0));
        assert_eq!(
            loaded.messages[0]
                .parameters
                .as_ref()
                .and_then(|parameters| parameters.max_output_tokens),
            Some(2048)
        );

        let mut kept = loaded.clone();
        assert!(kept.resolve_incomplete(0, IncompleteRecovery::Keep));
        assert_eq!(kept.messages[0].status, MessageStatus::Cancelled);
        assert!(matches!(
            kept.messages[0].parts.last(),
            Some(MessagePart::System { message }) if message == "recovery.kept_after_restart"
        ));

        let mut continued = loaded.clone();
        assert!(continued.resolve_incomplete(0, IncompleteRecovery::Continue));
        assert!(matches!(
            continued.messages[0].parts.last(),
            Some(MessagePart::System { message }) if message == "recovery.continued_after_restart"
        ));

        let mut discarded = loaded;
        assert!(discarded.resolve_incomplete(0, IncompleteRecovery::Discard));
        assert!(discarded.messages.is_empty());
        assert!(!discarded.resolve_incomplete(0, IncompleteRecovery::Discard));
    }
}
