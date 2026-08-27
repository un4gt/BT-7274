# MCP 远程连接

## 支持范围

BT-7274 只支持 MCP Streamable HTTP Transport：

- 客户端通过 HTTP(S) URL 建立远程连接。
- 服务端响应可以是 `application/json`，也可以是 `text/event-stream`。
- 支持 Bearer Token、自定义 Headers、完整 `${ENV_VAR}` 引用和全局代理。
- 支持新增、编辑、删除、启停、连接、重连、状态展示和退出清理。
- 初始化后保存协议版本、服务端名称/版本及 Resources/Prompts 能力标记。

不支持本地进程 Transport。MCP Runtime 当前也不会读取 Resources/Prompts 或调用任何远程
能力；连接仅用于远程服务可达性和能力元数据管理。

## 配置

推荐在“设置 -> MCP”中维护。对应 TOML：

```toml
[[mcp_servers]]
id = "docs"
name = "Documentation"
enabled = true
timeout_seconds = 60

[mcp_servers.transport]
kind = "streamable_http"
url = "https://mcp.example.com/mcp"
auth_token = "${MCP_TOKEN}"
headers = { "x-tenant" = "example" }
```

字段约束：

- `id` 以小写字母开头，只含小写字母、数字和下划线，最多 24 字符。
- `name` 最多 128 字符。
- `timeout_seconds` 范围为 1 到 3600 秒。
- `url` 必须是 HTTP(S) 地址，不能包含 userinfo 或 fragment。
- `auth_token` 填写裸 token，客户端负责生成 `Authorization: Bearer ...`。
- `headers` 不能覆盖 Authorization、Host、Content-Length、Accept、Content-Type、MCP
  Session 等协议保留字段。

`url`、`auth_token` 和 Header 值都可以使用完整环境变量引用，例如
`${MCP_TOKEN}`。缺失变量只报告变量名，不输出配置值。

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
4. 将状态更新为 `connected` 或 `failed`。
5. 监测连接是否意外关闭。

用户重连时遵循同一流程。旧 generation 的迟到事件会被忽略。

应用退出时，Runtime 请求关闭全部远程连接；总清理超过 8 秒时终止剩余连接任务。这里没有
本地服务进程需要回收。

## 状态

- `disabled`：配置存在但未启用。
- `starting`：正在初始化。
- `connected`：初始化成功且连接仍打开。
- `failed`：配置、网络、协议或连接状态失败。
- `stopping`：正在退出清理。

失败详情经过长度限制和凭据脱敏后显示。URL query、Bearer Token、自定义 Header 与代理
userinfo 中的凭据不会进入日志。

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
- HTTP 能返回但初始化失败：确认目标路径是 MCP Streamable HTTP 端点，而不是普通 REST
  或网页地址。

## 测试边界

本地 mock 覆盖 JSON initialize、能力摘要、状态变更、凭据脱敏和有界关闭。真实 Server 的
JSON/SSE、代理和鉴权组合仍应记录在发布 smoke matrix 中。
