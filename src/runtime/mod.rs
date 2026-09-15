//! 应用层使用的运行时边界。
//!
//! 该 facade 隔离 App/UI 与会话、模型、上下文和 MCP 连接实现。

/// 会话领域类型。持久化细节暂由旧 `session` 模块实现。
pub(crate) mod conversation {
    pub(crate) use crate::session::{IncompleteRecovery, Message, MessageStatus, Role, Session};
}

/// 上下文预算、分层统计与可撤销 compaction。
pub(crate) mod context;

/// Model 与 MCP 共用的错误分类和脱敏快照。
pub(crate) mod error;

/// 模型运行时入口：统一 Adapter、能力校验、HTTP/SSE 与原生协议实现。
pub(crate) mod model;

/// 远程 MCP Server 连接、状态和生命周期。
pub(crate) mod mcp;

/// 网络请求与后台连接共用的取消原语。
pub(crate) mod task;

/// Provider 与 MCP 之间共享的工具定义、调用和结果。
pub(crate) mod tool;
