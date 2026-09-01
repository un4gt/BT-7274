# MCP 远程连接

## 支持范围

BT-7274 只支持 MCP Streamable HTTP Transport：

- 客户端通过 HTTP(S) URL 建立远程连接。
- 服务端响应可以是 `application/json`，也可以是 `text/event-stream`。
- 支持环境变量 Bearer Token、静态/环境变量 Headers 和全局代理。
- 支持新增、编辑、删除、启停、连接、重连、状态展示和退出清理。
- 初始化后执行分页 `tools/list`，并展示协议版本、服务端名称/版本、工具数量及
  Resources/Prompts 能力标记。
- Chat Completions、Responses 和 Gemini GenerateContent 会把已连接 Server 的工具提供给
  模型，并执行模型返回的 `tools/call`。

不支持本地进程 Transport。Resources 和 Prompts 目前只展示能力标记，不读取其内容。
Anthropic Messages 尚未接入 MCP 工具调用。

## 配置

推荐在“设置 -> MCP”中维护。配置结构与 Codex CLI 一致：Server 名称就是
`mcp_servers` 下的表键，HTTP 配置直接位于该表内，不再重复保存 `id`、展示名或
`transport.kind`。完整字段命名可对照
[OpenAI 的 Codex MCP 文档](https://learn.chatgpt.com/docs/extend/mcp)。
也可以在聊天输入框输入 `/mcp` 直接打开该列表。

```toml
[mcp_servers.docs]
url = "https://mcp.example.com/mcp"
enabled = true
startup_timeout_sec = 60
bearer_token_env_var = "MCP_TOKEN"
http_headers = { "x-tenant" = "example" }
env_http_headers = { "x-api-key" = "MCP_API_KEY" }
```

字段约束：

- `docs` 是 Server 名称，不能为空、不能包含控制字符，最多 128 字符；名称重复会被拒绝。
- `url` 必须是 HTTP(S) 地址，不能包含 userinfo 或 fragment。
- `enabled` 默认为 `true`。
- `startup_timeout_sec` 默认为 `10`，范围为 1 到 3600 秒。
- `bearer_token_env_var` 填写环境变量名，例如 `MCP_TOKEN`，不能填写 token 或
  `${MCP_TOKEN}`。
- `http_headers` 保存静态 Header；`env_http_headers` 的值是环境变量名。
- 两类 Header 不能重名，也不能覆盖 Host、Content-Length、Accept、Content-Type、MCP
  Session 等协议保留字段。
- `bearer_token_env_var` 与显式 `Authorization` Header 不能同时配置。

MCP 配置只接受上述命名表结构。旧的 `[[mcp_servers]]`、`mcp_servers.transport`、
`auth_token` 和 `timeout_seconds` 不做兼容解析。此结构对应 `config_version = 8`；旧版本
配置会按全局配置策略备份，不会自动迁移。

环境变量缺失时只报告变量名，不输出配置值。静态 Header 值、环境 Header 的解析值、URL
query、Bearer Token 和代理凭据都会从运行时错误与日志中脱敏。

## 代理

MCP 使用应用的全局代理：

- HTTP/HTTPS：`http://host:port` 或 `https://host:port`
- SOCKS5：`socks5://host:port`
- SOCKS5H：`socks5h://host:port`

代理配置无效时，连接失败只影响对应 Server，TUI 继续运行。

## 生命周期

每次配置变更会为 Server 分配新 generation：

1. 取消并退休旧连接任务。
2. 校验并解析新配置。
3. 在超时范围内完成 MCP initialize。
4. 若 Server 声明 Tools 能力，在同一启动超时内完成分页 `tools/list`。
5. 将状态和工具目录更新为 `connected`，或在任一步失败时标记为 `failed`。
6. 接收模型运行时发来的 `tools/call`，并持续监测连接是否意外关闭。

用户重连时遵循同一流程。旧 generation 的迟到事件会被忽略。

应用退出时，Runtime 请求关闭全部远程连接；总清理超过 8 秒时终止剩余连接任务。这里没有
本地服务进程需要回收。

## 状态

- `disabled`：配置存在但未启用。
- `starting`：正在初始化。
- `connected`：初始化成功且连接仍打开。
- `failed`：配置、网络、协议或连接状态失败。
- `stopping`：正在退出清理。

失败详情经过长度限制和凭据脱敏后显示。能力元数据只保存在运行时状态中，不会写回
`config.toml`。

## 工具调用

连接成功的工具会生成稳定模型别名。别名只包含 ASCII 字母、数字、`_`、`-`，最多 64
字符，并包含 Server 名和稳定哈希后缀，从而实际避免不同 Server 的同名工具冲突。调用时
Runtime 再把别名映射回远端原名。

一次回复最多允许 8 个工具轮次，每轮最多 16 个调用。单次 `tools/call` 最长 60 秒，结果
上限为 256 KiB。远端工具错误会作为 `is_error` 结果返回模型，让模型有机会解释或改用其
他工具；用户取消、整体模型超时和应用退出会取消整条调用链。工具参数和结果不会写入日
志，但会作为独立的会话片段展示并在持久化前脱敏。

单个 Server 和单次模型请求最多各暴露 128 个工具；单个 input schema 最大 64 KiB。工具
调用由模型在生成期间自动发起，不提供逐次确认弹窗，因此只应配置你信任且权限范围合适
的 MCP Server。

## 诊断

PowerShell：

```powershell
$env:RUST_LOG = "bt_7274::runtime::mcp=debug"
cargo run --release
```

常见问题：

- `required environment variable ... is not available`：在启动程序的同一个 PowerShell
  会话中设置 `$env:变量名 = "值"`。
- `URL 必须是有效的 HTTP(S) 地址`：填写完整 scheme、host 和路径。
- `initialize exceeded ... seconds`：检查远程服务、代理和超时设置。
- `transport closed unexpectedly`：服务端主动断开或中间网络连接被关闭，可在设置页重连。
- `tools/list failed`：Server 声明了 Tools 能力，但工具目录不符合协议或网络请求失败。
- 工具已连接但模型不调用：确认当前协议是 Chat Completions、Responses 或 Gemini，并且
  所选模型本身支持 function calling。
- HTTP 能返回但初始化失败：确认目标路径是 MCP Streamable HTTP 端点，而不是普通 REST
  或网页地址。

## 测试边界

本地 mock 覆盖 JSON initialize、分页工具发现、实际工具调用、能力摘要、状态变更、凭据
脱敏和有界关闭。真实 Server 的 JSON/SSE、代理和鉴权组合仍应记录在发布 smoke matrix 中。
