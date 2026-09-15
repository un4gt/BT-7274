//! 上下文预算、保守 token 估算与可撤销压缩。

use chrono::Utc;

use crate::{
    config::{ContextSettings, ModelSettings},
    session::{
        BlockKind, CompactionRecord, CompactionSummary, Message, MessageStatus, Role, Session,
    },
};

const MESSAGE_OVERHEAD_TOKENS: usize = 4;
const KEEP_RECENT_MESSAGES: usize = 4;
const SUMMARY_SECTION_LIMIT: usize = 64;
const SUMMARY_LINE_CHARS: usize = 320;

/// 当前 Provider 没有暴露 tokenizer，因此预算明确标记为保守估算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenPrecision {
    Estimated,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextBreakdown {
    pub system: usize,
    pub conversation: usize,
}

impl ContextBreakdown {
    pub fn input_tokens(&self) -> usize {
        self.system.saturating_add(self.conversation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudget {
    pub precision: TokenPrecision,
    pub breakdown: ContextBreakdown,
    pub reserved_output: usize,
    pub context_window: Option<usize>,
    pub input_tokens: usize,
    pub planned_tokens: usize,
    pub remaining_tokens: Option<usize>,
    pub overflow_tokens: usize,
}

impl ContextBudget {
    pub fn usage_percent(&self) -> Option<u8> {
        let window = self.context_window.filter(|window| *window > 0)?;
        Some(
            self.planned_tokens
                .saturating_mul(100)
                .div_ceil(window)
                .min(100) as u8,
        )
    }

    pub fn overflowed(&self) -> bool {
        self.overflow_tokens > 0
    }

    pub fn should_auto_compact(&self, settings: &ContextSettings) -> bool {
        settings.auto_compact
            && self
                .usage_percent()
                .is_some_and(|percent| percent >= settings.auto_compact_threshold_percent)
    }

    pub fn add_system_tokens(&mut self, tokens: usize) {
        self.breakdown.system = self.breakdown.system.saturating_add(tokens);
        self.recalculate();
    }

    pub fn add_conversation_tokens(&mut self, tokens: usize) {
        self.breakdown.conversation = self.breakdown.conversation.saturating_add(tokens);
        self.recalculate();
    }

    fn recalculate(&mut self) {
        self.input_tokens = self.breakdown.input_tokens();
        self.planned_tokens = self.input_tokens.saturating_add(self.reserved_output);
        self.remaining_tokens = self
            .context_window
            .map(|window| window.saturating_sub(self.planned_tokens));
        self.overflow_tokens = self
            .context_window
            .map_or(0, |window| self.planned_tokens.saturating_sub(window));
    }
}

pub struct ContextInputs<'a> {
    pub system_instructions: &'a [&'a str],
    pub summary: Option<&'a CompactionSummary>,
    pub messages: &'a [Message],
}

impl<'a> ContextInputs<'a> {
    pub fn conversation(summary: Option<&'a CompactionSummary>, messages: &'a [Message]) -> Self {
        Self {
            system_instructions: &[],
            summary,
            messages,
        }
    }
}

pub fn build_budget(
    model: &ModelSettings,
    settings: &ContextSettings,
    inputs: ContextInputs<'_>,
) -> ContextBudget {
    let mut breakdown = ContextBreakdown {
        system: estimate_many(inputs.system_instructions),
        ..ContextBreakdown::default()
    };
    if let Some(summary) = inputs.summary {
        breakdown.conversation = breakdown
            .conversation
            .saturating_add(estimate_tokens(&summary.context_text()));
    }

    for message in inputs.messages {
        breakdown.conversation = breakdown
            .conversation
            .saturating_add(estimate_tokens(&message.content()))
            .saturating_add(MESSAGE_OVERHEAD_TOKENS);
        for block in &message.blocks {
            match &block.kind {
                BlockKind::Reasoning { content, .. } => {
                    breakdown.conversation = breakdown
                        .conversation
                        .saturating_add(estimate_tokens(content));
                }
                BlockKind::System { message } => {
                    breakdown.system = breakdown.system.saturating_add(estimate_tokens(message));
                }
                BlockKind::Text { .. } | BlockKind::Tool { .. } => {
                    // 已完成轮次的工具轨迹不重复发送给 Provider。
                }
            }
        }
    }

    let reserved_output = model
        .parameters
        .max_output_tokens
        .map(|tokens| tokens as usize)
        .unwrap_or_else(|| {
            let configured = settings.reserved_output_tokens as usize;
            model
                .capabilities
                .max_output_tokens
                .map(|limit| configured.min(limit as usize))
                .unwrap_or(configured)
        });
    let context_window = model
        .capabilities
        .context_window
        .map(|tokens| tokens as usize);
    let input_tokens = breakdown.input_tokens();
    let planned_tokens = input_tokens.saturating_add(reserved_output);
    let remaining_tokens = context_window.map(|window| window.saturating_sub(planned_tokens));
    let overflow_tokens = context_window
        .map(|window| planned_tokens.saturating_sub(window))
        .unwrap_or_default();

    ContextBudget {
        precision: TokenPrecision::Estimated,
        breakdown,
        reserved_output,
        context_window,
        input_tokens,
        planned_tokens,
        remaining_tokens,
        overflow_tokens,
    }
}

fn estimate_many(values: &[&str]) -> usize {
    values.iter().fold(0usize, |total, value| {
        total.saturating_add(estimate_tokens(value))
    })
}

/// 无 Provider tokenizer 时使用的保守估算：ASCII 字词约 4 字符/token，
/// CJK、emoji 与标点按 1 token 计。结果只用于提前预警和阻止确定的溢出。
pub fn estimate_tokens(text: &str) -> usize {
    let mut tokens = 0usize;
    let mut ascii_run = 0usize;
    let flush_ascii = |tokens: &mut usize, run: &mut usize| {
        if *run > 0 {
            *tokens = (*tokens).saturating_add((*run).div_ceil(4));
            *run = 0;
        }
    };

    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
            ascii_run += 1;
        } else {
            flush_ascii(&mut tokens, &mut ascii_run);
            if !character.is_whitespace() {
                tokens = tokens.saturating_add(1);
            }
        }
    }
    flush_ascii(&mut tokens, &mut ascii_run);
    if text.is_empty() { 0 } else { tokens.max(1) }
}

#[derive(Debug, Clone)]
pub struct CompactionPlan {
    pub automatic: bool,
    pub archive_count: usize,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub summary: CompactionSummary,
}

impl CompactionPlan {
    pub fn apply(self, session: &mut Session) {
        let archived_messages: Vec<Message> = session
            .messages
            .drain(..self.archive_count.min(session.messages.len()))
            .collect();

        let previous_summary = session.summary.clone();
        session.summary = (!self.summary.is_empty()).then_some(self.summary);
        session.compactions.push(CompactionRecord {
            id: format!(
                "{}-{}",
                Utc::now().format("%Y%m%dT%H%M%S%3f"),
                uuid::Uuid::new_v4().simple()
            ),
            created_at: Utc::now(),
            automatic: self.automatic,
            previous_summary,
            archived_messages,
        });
        session.updated_at = Utc::now();
    }
}

pub fn prepare_compaction(
    session: &Session,
    model: &ModelSettings,
    settings: &ContextSettings,
    automatic: bool,
) -> Option<CompactionPlan> {
    let first_streaming = session
        .messages
        .iter()
        .position(|message| message.status == MessageStatus::Streaming)
        .unwrap_or(session.messages.len());
    let mut archive_count = first_streaming.saturating_sub(KEEP_RECENT_MESSAGES);
    while archive_count > 0
        && session
            .messages
            .get(archive_count)
            .is_some_and(|message| message.role != Role::User)
    {
        archive_count -= 1;
    }
    if archive_count < 2 {
        archive_count = 0;
    }

    let mut summary = session.summary.clone().unwrap_or_default();
    summarize_messages(&mut summary, &session.messages[..archive_count]);

    if archive_count == 0 {
        return None;
    }

    let before_tokens = build_budget(
        model,
        settings,
        ContextInputs::conversation(session.summary.as_ref(), &session.messages),
    )
    .input_tokens;
    let mut plan = CompactionPlan {
        automatic,
        archive_count,
        before_tokens,
        after_tokens: before_tokens,
        summary,
    };
    let mut simulated = session.clone();
    plan.clone().apply(&mut simulated);
    plan.after_tokens = build_budget(
        model,
        settings,
        ContextInputs::conversation(simulated.summary.as_ref(), &simulated.messages),
    )
    .input_tokens;
    Some(plan)
}

pub fn undo_last_compaction(session: &mut Session) -> bool {
    let Some(record) = session.compactions.pop() else {
        return false;
    };
    let mut restored = record.archived_messages;
    restored.append(&mut session.messages);
    session.messages = restored;
    session.summary = record.previous_summary;
    session.updated_at = Utc::now();
    true
}

fn summarize_messages(summary: &mut CompactionSummary, messages: &[Message]) {
    for message in messages {
        let role = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        for line in message.content().lines() {
            classify_summary_line(summary, role, line);
        }
        for block in &message.blocks {
            match &block.kind {
                BlockKind::Text { .. } => {}
                BlockKind::Reasoning { content, .. } => {
                    for line in content.lines() {
                        classify_summary_line(summary, "Reasoning", line);
                    }
                }
                BlockKind::System { message } => {
                    classify_summary_line(summary, "System", message);
                }
                BlockKind::Tool { tool } => {
                    let arguments = tool
                        .arguments
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| tool.arguments_text.clone());
                    classify_summary_line(
                        summary,
                        "Tool call",
                        &format!("{}/{} {arguments}", tool.server, tool.name),
                    );
                    if let Some(output) = &tool.output {
                        classify_summary_line(
                            summary,
                            if tool.status == crate::session::ToolStatus::Failed {
                                "Tool error"
                            } else {
                                "Tool result"
                            },
                            &format!("{}/{} {output}", tool.server, tool.name),
                        );
                    }
                }
            }
        }
    }
}

fn classify_summary_line(summary: &mut CompactionSummary, role: &str, raw: &str) {
    let line = raw.trim();
    if line.is_empty() {
        return;
    }
    let compact = truncate_chars(line, SUMMARY_LINE_CHARS);
    let entry = format!("{role}: {compact}");
    let lower = line.to_lowercase();
    let mut matched = false;
    if [
        "decision", "decided", "choose", "selected", "决定", "采用", "选用",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
    {
        push_unique(&mut summary.decisions, entry.clone());
        matched = true;
    }
    if [
        "must",
        "must not",
        "never",
        "only",
        "constraint",
        "禁止",
        "必须",
        "不要",
        "只能",
        "约束",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
    {
        push_unique(&mut summary.constraints, entry.clone());
        matched = true;
    }
    if [
        "implemented",
        "changed",
        "fixed",
        "added",
        "removed",
        ".rs",
        ".toml",
        ".md",
        "实现",
        "修改",
        "修复",
        "新增",
        "删除",
        "代码",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
    {
        push_unique(&mut summary.code_state, entry.clone());
        matched = true;
    }
    if [
        "todo",
        "next",
        "pending",
        "unfinished",
        "待办",
        "下一步",
        "未完成",
        "继续",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
    {
        push_unique(&mut summary.todos, entry.clone());
        matched = true;
    }
    if [
        "http://",
        "https://",
        "src/",
        "docs/",
        "tests/",
        "request id",
        "参考",
        "路径",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
    {
        push_unique(&mut summary.references, entry.clone());
        matched = true;
    }
    if !matched {
        push_unique(&mut summary.notes, entry);
    }
}

fn push_unique(target: &mut Vec<String>, value: String) {
    if target.iter().any(|existing| existing == &value) {
        return;
    }
    if target.len() == SUMMARY_SECTION_LIMIT {
        target.remove(0);
    }
    target.push(value);
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelCapabilities, ModelParameters, ModelSelection};

    fn message(role: Role, content: &str) -> Message {
        match role {
            Role::User => Message::user(content.to_owned(), None),
            Role::Assistant => {
                let mut message = Message::assistant_streaming(ModelSelection {
                    provider_id: "provider".to_owned(),
                    provider_name: "Provider".to_owned(),
                    model: "model".to_owned(),
                });
                message.append_text(content);
                message.status = MessageStatus::Completed;
                message
            }
        }
    }

    #[test]
    fn estimator_is_unicode_aware_and_explicitly_conservative() {
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("中文测试"), 4);
        assert!(estimate_tokens("fn main() { println!(\"你好\"); }") >= 8);
    }

    #[test]
    fn budget_keeps_system_conversation_and_reserved_output_separate() {
        let model = ModelSettings {
            capabilities: ModelCapabilities {
                context_window: Some(100),
                max_output_tokens: Some(20),
                ..ModelCapabilities::default()
            },
            parameters: ModelParameters {
                max_output_tokens: Some(10),
                ..ModelParameters::default()
            },
        };
        let messages = vec![message(Role::User, "hello")];
        let budget = build_budget(
            &model,
            &ContextSettings::default(),
            ContextInputs {
                system_instructions: &["system"],
                summary: None,
                messages: &messages,
            },
        );
        assert_eq!(budget.reserved_output, 10);
        assert_eq!(budget.planned_tokens, budget.input_tokens + 10);
        assert!(budget.breakdown.system > 0);
        assert!(budget.breakdown.conversation > 0);
        assert!(!budget.overflowed());
    }

    #[test]
    fn budget_reports_threshold_and_blocks_known_overflow() {
        let model = ModelSettings {
            capabilities: ModelCapabilities {
                context_window: Some(40),
                ..ModelCapabilities::default()
            },
            parameters: ModelParameters {
                max_output_tokens: Some(16),
                ..ModelParameters::default()
            },
        };
        let messages = vec![message(
            Role::User,
            "这是一个很长的中文上下文，用于确保预算不会继续使用简单的字符数除以四。",
        )];
        let settings = ContextSettings {
            auto_compact_threshold_percent: 60,
            ..ContextSettings::default()
        };
        let budget = build_budget(
            &model,
            &settings,
            ContextInputs::conversation(None, &messages),
        );
        assert!(budget.should_auto_compact(&settings));
        assert!(budget.overflowed());
        assert!(budget.overflow_tokens > 0);
        assert_eq!(budget.remaining_tokens, Some(0));
    }

    #[test]
    fn compacted_summary_stays_in_conversation_not_system_layer() {
        let summary = CompactionSummary {
            constraints: vec!["User: 必须保留 Windows 支持".to_owned()],
            ..CompactionSummary::default()
        };
        let budget = build_budget(
            &ModelSettings::default(),
            &ContextSettings::default(),
            ContextInputs::conversation(Some(&summary), &[]),
        );
        assert_eq!(budget.breakdown.system, 0);
        assert!(budget.breakdown.conversation > 0);
    }

    #[test]
    fn compaction_preserves_multilingual_task_state_and_is_reversible() {
        let mut session = Session::new();
        session.messages = vec![
            message(Role::User, "决定采用 Rust；必须兼容 Windows。"),
            message(Role::Assistant, "已修改 src/app.rs。"),
            message(
                Role::User,
                "TODO: 下一步修复搜索；参考 https://example.test/spec",
            ),
            message(Role::Assistant, "收到，待办仍未完成。"),
            message(Role::User, "keep recent user"),
            message(Role::Assistant, "keep recent assistant"),
            message(Role::User, "latest question"),
            message(Role::Assistant, "latest answer"),
        ];
        let original = session.messages.clone();
        let model = ModelSettings::default();
        let plan =
            prepare_compaction(&session, &model, &ContextSettings::default(), false).unwrap();
        let preview = plan.summary.context_text();
        assert!(preview.contains("决定采用 Rust"));
        assert!(preview.contains("必须兼容 Windows"));
        assert!(preview.contains("src/app.rs"));
        assert!(preview.contains("TODO"));
        assert!(preview.contains("https://example.test/spec"));

        plan.apply(&mut session);
        assert!(session.messages.len() < original.len());
        assert!(undo_last_compaction(&mut session));
        assert_eq!(session.messages, original);
        assert!(session.summary.is_none());
    }

    #[test]
    fn long_code_conversation_keeps_early_facts_todos_and_latest_turn() {
        let mut session = Session::new();
        session.messages.push(message(
            Role::User,
            "Decision: use atomic writes. Constraint: never delete the destination first. TODO: add crash recovery. Reference: docs/architecture.md",
        ));
        session.messages.push(message(
            Role::Assistant,
            "Implemented src/storage.rs and kept the Windows ReplaceFileW path.",
        ));
        for index in 0..36 {
            session.messages.push(message(
                Role::User,
                &format!("routine multilingual turn {index}: 检查状态 {index}"),
            ));
            session.messages.push(message(
                Role::Assistant,
                &format!("routine answer {index}: no task-state change"),
            ));
        }
        session.messages.push(message(
            Role::User,
            "latest user requirement must remain active",
        ));
        session.messages.push(message(
            Role::Assistant,
            "latest assistant state remains active",
        ));
        let original = session.messages.clone();
        let plan = prepare_compaction(
            &session,
            &ModelSettings::default(),
            &ContextSettings::default(),
            false,
        )
        .unwrap();
        let summary = plan.summary.context_text();
        for fact in [
            "atomic writes",
            "never delete",
            "TODO",
            "docs/architecture.md",
            "src/storage.rs",
            "ReplaceFileW",
        ] {
            assert!(summary.contains(fact), "missing compacted fact: {fact}");
        }
        plan.apply(&mut session);
        assert!(
            session
                .messages
                .iter()
                .any(|message| message.content().contains("latest user requirement"))
        );
        assert!(
            session
                .messages
                .iter()
                .any(|message| message.content().contains("latest assistant state"))
        );
        assert!(undo_last_compaction(&mut session));
        assert_eq!(session.messages, original);
    }

    #[test]
    fn compaction_never_archives_an_unfinished_generation() {
        let mut session = Session::new();
        for index in 0..8 {
            session.messages.push(message(
                if index % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                &format!("completed {index}"),
            ));
        }
        let selection = ModelSelection {
            provider_id: "provider".to_owned(),
            provider_name: "Provider".to_owned(),
            model: "model".to_owned(),
        };
        let mut streaming = Message::assistant_streaming(selection);
        streaming.append_text("partial");
        session.messages.push(streaming);
        let plan = prepare_compaction(
            &session,
            &ModelSettings::default(),
            &ContextSettings::default(),
            false,
        )
        .unwrap();
        plan.apply(&mut session);
        assert!(
            session
                .messages
                .iter()
                .any(|message| message.status == MessageStatus::Streaming)
        );
    }
}
