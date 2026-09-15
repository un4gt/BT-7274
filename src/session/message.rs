//! Ordered conversation blocks. Provider payloads and view state never become answer text.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{MessageStatus, Role};
use crate::config::{ModelParameters, ModelSelection};
use crate::runtime::error::RuntimeErrorSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Preparing,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Incomplete,
}

impl ToolStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Preparing | Self::Queued | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolActivity {
    pub round: usize,
    pub index: usize,
    pub call_id: String,
    pub server: String,
    pub name: String,
    /// Only incomplete arguments live here; validated arguments replace them atomically.
    pub arguments_text: String,
    pub arguments: Option<Value>,
    pub output: Option<Value>,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockKind {
    Text {
        content: String,
    },
    Reasoning {
        content: String,
        status: MessageStatus,
    },
    Tool {
        tool: ToolActivity,
    },
    System {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageBlock {
    /// Monotonic within a message; completion updates never move or replace a block.
    pub id: u64,
    #[serde(default)]
    pub revision: u64,
    #[serde(flatten)]
    pub kind: BlockKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "MessageWire")]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub blocks: Vec<MessageBlock>,
    #[serde(default)]
    pub legacy_order: bool,
    pub model: Option<ModelSelection>,
    pub parameters: Option<ModelParameters>,
    pub status: MessageStatus,
    pub failure: Option<RuntimeErrorSnapshot>,
}

// Read the old format without retaining a second, mutable copy of the conversation.
#[derive(Deserialize)]
struct MessageWire {
    #[serde(default = "new_id")]
    id: String,
    role: Role,
    #[serde(default)]
    blocks: Option<Vec<MessageBlock>>,
    #[serde(default)]
    content: String,
    #[serde(default)]
    parts: Vec<LegacyPart>,
    #[serde(default)]
    legacy_order: bool,
    #[serde(default)]
    model: Option<ModelSelection>,
    #[serde(default)]
    parameters: Option<ModelParameters>,
    #[serde(default)]
    status: MessageStatus,
    #[serde(default)]
    failure: Option<RuntimeErrorSnapshot>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyPart {
    Reasoning {
        content: String,
    },
    System {
        message: String,
    },
    ToolCall {
        call_id: String,
        server: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        server: String,
        name: String,
        output: Value,
        is_error: bool,
    },
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

impl From<MessageWire> for Message {
    fn from(wire: MessageWire) -> Self {
        let mut message = Self {
            id: wire.id,
            role: wire.role,
            blocks: Vec::new(),
            legacy_order: wire.legacy_order,
            model: wire.model,
            parameters: wire.parameters,
            status: wire.status,
            failure: wire.failure,
        };
        if let Some(blocks) = wire.blocks {
            message.blocks = blocks;
        } else {
            message.legacy_order = !wire.parts.is_empty();
            message.append_text(&wire.content);
            for part in wire.parts {
                match part {
                    LegacyPart::Reasoning { content } => message.append_reasoning(&content),
                    LegacyPart::System { message: text } => message.push_system_event(text),
                    LegacyPart::ToolCall {
                        call_id,
                        server,
                        name,
                        arguments,
                    } => message.push_tool_call(call_id, server, name, arguments),
                    LegacyPart::ToolResult {
                        call_id,
                        server,
                        name,
                        output,
                        is_error,
                    } => message.push_tool_result(call_id, server, name, output, is_error),
                }
            }
            message.finish(wire.status);
        }
        message
    }
}

impl Message {
    pub fn new(role: Role, content: String, model: Option<ModelSelection>) -> Self {
        let mut message = Self {
            id: new_id(),
            role,
            blocks: Vec::new(),
            legacy_order: false,
            model,
            parameters: None,
            status: MessageStatus::Completed,
            failure: None,
        };
        message.append_text(&content);
        message
    }

    pub fn user(content: String, model: Option<ModelSelection>) -> Self {
        Self::new(Role::User, content, model)
    }

    pub fn user_with_request(
        content: String,
        model: ModelSelection,
        parameters: ModelParameters,
    ) -> Self {
        let mut message = Self::user(content, Some(model));
        message.parameters = Some(parameters);
        message
    }

    pub fn assistant_streaming(model: ModelSelection) -> Self {
        let mut message = Self::new(Role::Assistant, String::new(), Some(model));
        message.status = MessageStatus::Streaming;
        message
    }

    pub fn assistant_streaming_with_request(
        model: ModelSelection,
        parameters: ModelParameters,
    ) -> Self {
        let mut message = Self::assistant_streaming(model);
        message.parameters = Some(parameters);
        message
    }

    pub fn content(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Text { content } => Some(content.as_str()),
                _ => None,
            })
            .collect()
    }

    fn push(&mut self, kind: BlockKind) -> &mut MessageBlock {
        let id = self.blocks.last().map_or(1, |block| block.id + 1);
        self.blocks.push(MessageBlock {
            id,
            revision: 0,
            kind,
        });
        self.blocks.last_mut().expect("just pushed a block")
    }

    fn finish_reasoning(&mut self, status: MessageStatus) {
        for block in &mut self.blocks {
            if let BlockKind::Reasoning {
                status: current, ..
            } = &mut block.kind
                && *current == MessageStatus::Streaming
            {
                *current = status;
                block.revision += 1;
            }
        }
    }

    pub fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.finish_reasoning(MessageStatus::Completed);
        let last = self
            .blocks
            .iter_mut()
            .rev()
            .find(|b| !matches!(b.kind, BlockKind::System { .. }));
        if let Some(MessageBlock {
            kind: BlockKind::Text { content },
            revision,
            ..
        }) = last
        {
            content.push_str(text);
            *revision += 1;
        } else {
            self.push(BlockKind::Text {
                content: text.to_owned(),
            });
        }
    }

    pub fn append_reasoning(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let last = self
            .blocks
            .iter_mut()
            .rev()
            .find(|b| !matches!(b.kind, BlockKind::System { .. }));
        if let Some(MessageBlock {
            kind:
                BlockKind::Reasoning {
                    content,
                    status: MessageStatus::Streaming,
                },
            revision,
            ..
        }) = last
        {
            content.push_str(text);
            *revision += 1;
        } else {
            self.push(BlockKind::Reasoning {
                content: text.to_owned(),
                status: self.status,
            });
        }
    }

    pub fn push_system_event(&mut self, message: String) {
        if !message.is_empty() {
            self.push(BlockKind::System { message });
        }
    }

    fn tool_mut(&mut self, round: usize, index: usize) -> &mut ToolActivity {
        let position = self.blocks.iter().position(|b| {
            matches!(&b.kind,
            BlockKind::Tool { tool } if tool.round == round && tool.index == index)
        });
        let position = position.unwrap_or_else(|| {
            self.finish_reasoning(MessageStatus::Completed);
            self.push(BlockKind::Tool {
                tool: ToolActivity {
                    round,
                    index,
                    call_id: String::new(),
                    server: String::new(),
                    name: String::new(),
                    arguments_text: String::new(),
                    arguments: None,
                    output: None,
                    status: ToolStatus::Preparing,
                },
            });
            self.blocks.len() - 1
        });
        let block = &mut self.blocks[position];
        block.revision += 1;
        let BlockKind::Tool { tool } = &mut block.kind else {
            unreachable!()
        };
        tool
    }

    pub fn tool_delta(
        &mut self,
        round: usize,
        index: usize,
        call_id: String,
        name: String,
        arguments: String,
        replace: bool,
    ) {
        let tool = self.tool_mut(round, index);
        if !tool.status.is_active() {
            return;
        }
        tool.call_id = call_id;
        tool.name = name;
        if replace {
            tool.arguments_text = arguments;
        } else {
            tool.arguments_text.push_str(&arguments);
        }
    }

    pub fn tool_ready(
        &mut self,
        round: usize,
        index: usize,
        call_id: String,
        server: String,
        name: String,
        arguments: Value,
    ) {
        let tool = self.tool_mut(round, index);
        tool.call_id = call_id;
        tool.server = server;
        tool.name = name;
        tool.arguments_text.clear();
        tool.arguments = Some(arguments);
        tool.status = ToolStatus::Queued;
    }

    pub fn tool_started(&mut self, round: usize, index: usize) {
        self.tool_mut(round, index).status = ToolStatus::Running;
    }

    pub fn tool_result(&mut self, round: usize, index: usize, output: Value, is_error: bool) {
        let tool = self.tool_mut(round, index);
        tool.output = Some(output);
        tool.status = if is_error {
            ToolStatus::Failed
        } else {
            ToolStatus::Succeeded
        };
    }

    // Legacy import and fixtures use call IDs, matching the nearest unfinished call.
    pub fn push_tool_call(
        &mut self,
        call_id: String,
        server: String,
        name: String,
        arguments: Value,
    ) {
        let index = self.blocks.len();
        self.tool_ready(0, index, call_id, server, name, arguments);
        self.tool_started(0, index);
    }

    pub fn push_tool_result(
        &mut self,
        call_id: String,
        server: String,
        name: String,
        output: Value,
        is_error: bool,
    ) {
        let key = self.blocks.iter().rev().find_map(|b| match &b.kind {
            BlockKind::Tool { tool } if tool.call_id == call_id && tool.output.is_none() => {
                Some((tool.round, tool.index))
            }
            _ => None,
        });
        let (round, index) = key.unwrap_or_else(|| {
            let index = self.blocks.len();
            let tool = self.tool_mut(0, index);
            tool.call_id = call_id;
            tool.server = server;
            tool.name = name;
            (0, index)
        });
        self.tool_result(round, index, output, is_error);
    }

    pub fn finish(&mut self, status: MessageStatus) {
        self.status = status;
        if status == MessageStatus::Streaming {
            return;
        }
        self.finish_reasoning(status);
        for block in &mut self.blocks {
            if let BlockKind::Tool { tool } = &mut block.kind
                && tool.status.is_active()
            {
                tool.status = match status {
                    MessageStatus::Cancelled => ToolStatus::Cancelled,
                    MessageStatus::Failed => ToolStatus::Failed,
                    _ => ToolStatus::Incomplete,
                };
                block.revision += 1;
            }
        }
    }

    pub fn recover_blocks(&mut self) {
        self.finish_reasoning(MessageStatus::Cancelled);
        for block in &mut self.blocks {
            if let BlockKind::Tool { tool } = &mut block.kind
                && tool.status.is_active()
            {
                tool.status = ToolStatus::Incomplete;
                block.revision += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_stream_order_round_identity_and_legacy_history() {
        let mut message = Message::new(Role::Assistant, String::new(), None);
        message.status = MessageStatus::Streaming;
        message.append_reasoning("considering");
        message.append_text("before ");
        message.push_system_event("response.queued".to_owned());
        message.append_text("tools");
        message.tool_delta(
            0,
            2,
            "same-id".to_owned(),
            "search".to_owned(),
            "{\"q\":".to_owned(),
            false,
        );
        message.tool_delta(
            0,
            5,
            "another".to_owned(),
            "read".to_owned(),
            "{}".to_owned(),
            false,
        );
        message.tool_delta(
            0,
            2,
            "same-id".to_owned(),
            "search".to_owned(),
            "\"rust\"}".to_owned(),
            false,
        );
        let first_id = message
            .blocks
            .iter()
            .find(|b| matches!(&b.kind, BlockKind::Tool { tool } if tool.index == 2))
            .unwrap()
            .id;
        message.tool_ready(
            0,
            2,
            "same-id".to_owned(),
            "docs".to_owned(),
            "search".to_owned(),
            serde_json::json!({"q":"rust"}),
        );
        message.tool_started(0, 2);
        message.tool_result(0, 2, serde_json::json!({"answer":"found"}), false);
        message.append_text(" after tools");
        message.tool_delta(
            1,
            2,
            "same-id".to_owned(),
            "search".to_owned(),
            "{".to_owned(),
            false,
        );
        message.finish(MessageStatus::Cancelled);
        assert_eq!(message.content(), "before tools after tools");
        let calls = message
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Tool { tool } => Some((block.id, tool)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, first_id);
        assert_eq!(calls[0].1.status, ToolStatus::Succeeded);
        assert_eq!(calls[0].1.output.as_ref().unwrap()["answer"], "found");
        assert_eq!(calls[1].1.status, ToolStatus::Cancelled);
        assert_eq!(calls[2].1.status, ToolStatus::Cancelled);
        assert_ne!(calls[0].0, calls[2].0);
        let positions = message
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Text { content } => Some(content.as_str()),
                BlockKind::Tool { .. } => Some("tool"),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            positions,
            ["before tools", "tool", "tool", " after tools", "tool"]
        );
        let restored: Message =
            serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert_eq!(restored, message);

        let legacy: Message = serde_json::from_value(serde_json::json!({
            "role":"assistant", "content":"saved answer", "parts":[
                {"type":"reasoning","content":"saved thought"},
                {"type":"tool_call","call_id":"old","server":"docs","name":"search","arguments":{"q":"old"}},
                {"type":"tool_result","call_id":"old","server":"docs","name":"search","output":{"text":"saved result"},"is_error":false}
            ]
        })).unwrap();
        assert!(legacy.legacy_order);
        assert_eq!(legacy.content(), "saved answer");
        assert_eq!(
            legacy
                .blocks
                .iter()
                .filter(|b| matches!(b.kind, BlockKind::Tool { .. }))
                .count(),
            1
        );
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert!(encoded.get("parts").is_none());
        assert!(encoded.get("content").is_none());
        assert_eq!(serde_json::from_value::<Message>(encoded).unwrap(), legacy);
    }
}
