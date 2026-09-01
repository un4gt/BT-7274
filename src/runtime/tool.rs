//! 模型 Provider 与 MCP Runtime 之间的统一工具调用契约。

use serde_json::Value;

/// 一个可暴露给模型的 MCP 工具。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    /// 发给模型的稳定别名，符合 OpenAI/Gemini 的函数命名限制。
    pub model_name: String,
    pub server_name: String,
    pub remote_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// 模型请求执行一次工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    /// Provider 返回的原始调用 ID；协议允许缺失时与内部 `id` 分开保存。
    pub provider_id: Option<String>,
    /// `ToolDefinition::model_name`。
    pub name: String,
    pub arguments: Value,
}

/// MCP 工具调用的标准化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub output: Value,
    pub is_error: bool,
}

/// 一次模型工具轮次；下一次 Provider 请求必须原样带回调用及结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRound {
    pub calls: Vec<ToolCall>,
    pub results: Vec<ToolResult>,
    /// 当前 Provider 要求在下一轮原样回传的 assistant/model 输出。
    pub provider_state: Option<Value>,
}
