# BT-7274 架构

## 产品边界

BT-7274 是 TUI Chat 客户端，不是代码工作台或自主任务运行时。核心职责只有：

1. 管理 Provider、Model、代理和远程 MCP 连接配置。
2. 将对话编码为 Provider 原生请求并消费流式响应。
3. 管理会话、上下文预算、压缩、恢复和本地持久化。
4. 在终端中提供稳定、可取消、可诊断的聊天体验。

程序不访问用户项目目录，也不运行本地用户命令。仅当用户配置并启用远程 MCP Server 时，
模型可以请求执行该 Server 公布的工具。

## 模块

```text
src/
├── main.rs                 # 启动、终端恢复、日志初始化
├── app.rs                  # TUI 状态与交互协调
├── config.rs               # 当前版本配置、Provider、Model、Proxy、MCP
├── session.rs              # 会话、消息、压缩记录、JSON 持久化
├── editor.rs               # 多行聊天输入编辑器
├── event.rs                # 终端和后台事件通道
├── logging.rs              # 按日期文件日志与过滤
├── secret.rs               # 环境变量引用和脱敏
├── storage.rs              # 私有数据原子写入
├── runtime/
│   ├── context.rs          # token 估算、预算与可撤销压缩
│   ├── error.rs            # 运行时错误分类
│   ├── task.rs             # 网络任务取消原语
│   ├── tool.rs             # Provider 与 MCP 共用的工具契约
│   ├── model/              # Provider adapters、HTTP/SSE、重试
│   └── mcp/                # 远程连接、工具发现与 actor 调用通道
└── ui/                     # Ratatui 渲染组件
```

## 对话数据流

```text
键盘输入
  -> ChatEditor
  -> App::start_turn
  -> Context 预算与可选压缩
  -> Provider Adapter 原生请求
  -> HTTP 响应 / SSE decoder
  -> 可选工具调用 -> MCP Registry -> Server actor -> tools/call
  -> 工具结果回送 Provider，继续下一轮
  -> ModelStreamEvent
  -> App 更新目标会话
  -> 原子保存会话
  -> TUI 重绘
```

`ModelStreamEvent` 只表达聊天所需语义：

- `AssistantTextDelta`
- `ReasoningDelta`
- `System`
- `ToolCall`
- `ToolResult`
- `Completed`
- `Error`
- `Cancelled`

流事件携带 `stream id`。取消或切换后迟到的事件因 id 不匹配而被丢弃，不会写入错误
会话。

## Provider Adapter

四种协议共享统一边界：

- 构建原生请求和鉴权。
- 将正文、reasoning、usage、完成原因和错误映射为统一事件。
- 复用 HTTP client、代理、超时、有限重试、`Retry-After` 和 SSE framing。
- 记录 request id、TTFT、总耗时、输出 token 和终态。

会话历史只编码 user/assistant 正文。摘要作为 user 级 Conversation 数据发送，不进入
system instructions。Chat Completions、Responses 和 Gemini 在当前生成链内额外编码统一工
具目录与工具轮次；Anthropic 暂不暴露工具。

## 会话与上下文

`Message` 保存：

- `role` 与正文。
- reasoning、系统事件、MCP 调用和结果片段。
- 实际 Provider/Model 与请求参数快照。
- completed/streaming/cancelled/failed 状态。
- 脱敏后的失败分类摘要。

上下文预算只包含 System、Conversation 和 Reserved Output。模型窗口已知时，请求前阻止
确定溢出；MCP 工具定义计入 System，当前工具轮次计入 Conversation。没有统一 tokenizer
时使用明确标记的保守估算。

压缩归档旧消息并生成结构化摘要。归档记录保留在会话 JSON 中，因此最近一次压缩可撤销。

## 持久化

配置和会话写入目标目录内的唯一临时文件，完成 `flush` 与 `sync_all` 后原子替换目标。
Unix 私有文件权限设为 `0600`。单个损坏会话只产生脱敏警告，不阻断其余会话加载。

配置只接受当前 `config_version`，不运行迁移链；不兼容文件原样备份后使用默认配置启动。

## MCP

MCP Runtime 只支持远程 Streamable HTTP。连接流程为：

```text
配置校验
  -> 解析环境变量引用
  -> 应用全局 HTTP/SOCKS 代理
  -> initialize
  -> tools/list
  -> 保存协议/服务端/能力摘要和工具目录
  -> actor 接收 tools/call / 监测连接状态
  -> 用户重连或有界关闭
```

服务端可用 JSON 或 SSE 返回协议响应。每个 Server actor 独占 `RunningService`，Registry
只保存工具目录和命令 sender，避免在锁内跨 `await`。每个 Server 有独立 generation 和取
消 token；旧连接产生的迟到状态不会覆盖新连接。应用退出时先请求关闭，超时后终止连接
任务。

模型 Runtime 运行有界工具循环，执行结果通过统一 `ToolRound` 按 Provider 原生格式回送。
Resources/Prompts 仍只展示能力标记。详细配置见 [mcp.md](mcp.md)。

## 终端生命周期

主循环仅在生成、模型同步、MCP 状态过渡或可见动画期间启用 tick，空闲时等待真实事件。
正常退出和错误退出都恢复 raw mode、alternate screen、光标和 panic hook。

## 安全边界

- API Key、敏感 Header、代理凭据和 MCP 凭据在日志与 Debug 输出中脱敏。
- 环境变量引用只在运行时解析，不回写解析值。
- 不记录聊天正文、请求正文或 Provider 响应正文。
- 不记录 MCP 工具参数或结果；会话写盘前对结构化 JSON 递归脱敏。
- MCP URL 只接受 HTTP(S)，禁止 URL userinfo、fragment 和协议保留 Header 覆盖。
- 自定义 Provider 参数不能覆盖消息、流、模型或鉴权结构字段。
