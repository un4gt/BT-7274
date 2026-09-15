# BT-7274

BT-7274 是一个基于 Rust 与 Ratatui 的终端 AI Chat 客户端。它专注于聊天、会话、Provider、
上下文与终端交互，不包含代码代理、本地命令执行、Skills、Sub-agent 或 stdio MCP。

## 核心功能

- OpenAI Chat Completions、OpenAI Responses、Anthropic Messages、Gemini GenerateContent
- 多 Provider 新增、编辑、删除、模型同步与手动模型维护
- 正文与 reasoning 流式展示，取消后保留部分回复
- Markdown、代码查看器、多行编辑、选择与 OSC 52 复制
- 鼠标点击查看工具结果、切换会话，滚轮浏览消息与过程详情，输入框点击定位与拖选
- 会话持久化、崩溃恢复、上下文预算与可撤销压缩
- HTTP/HTTPS、SOCKS5/SOCKS5H 全局代理
- Vanguard、Carbon、Paper 三套主题与中英双语
- 输入框星空特效，支持淡入淡出与 `whimsy` 运行时开关
- 按日期写入 `~/.bt7274/logs/YYYY-MM-DD.log` 的脱敏日志
- 可选远程 Streamable HTTP MCP：工具发现与调用可用于 Chat Completions、Responses 和 Gemini

## 安装

Windows PowerShell：

```powershell
irm https://github.com/un4gt/BT-7274/releases/latest/download/bt-7274-installer.ps1 | iex
```

macOS / Linux：

```shell
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/un4gt/BT-7274/releases/latest/download/bt-7274-installer.sh | sh
```

从源码运行：

```shell
cargo run --release
```

聊天输入框支持 `/models` 打开模型列表、`/mcp` 打开已配置 MCP Server 列表。

查看工具调用结果：点击消息中的“▸ … 项工具”过程记录，或按 `F3` 打开过程详情。
选择工具后，可点击“参数 / 结果 / 原始”标签切换内容，滚轮浏览，按 `c` 复制完整内容，
点击“关闭”或按 `Esc` 返回。输入框支持点击定位光标、拖动选择文本及 `Alt+C` 复制选择。

## Gemini 快速配置

PowerShell 中先设置 API Key：

```powershell
$env:GEMINI_API_KEY = "你的 API Key"
```

将以下内容保存到系统配置目录的 `bt-7274/config.toml`：

```toml
config_version = 8
current_provider = 0
model = "gemini-2.5-flash"
language = "zh"
theme = "vanguard"
show_titan = false
whimsy = false

[[providers]]
id = "provider-gemini"
name = "Gemini"
api_kind = "gemini_generate_content"
models = ["gemini-2.5-flash"]
api_key = "${GEMINI_API_KEY}"

[proxy]
mode = "disabled"
```

官方 Base URL 可以省略。BT-7274 会自动添加
`x-goog-api-client: bt-7274/0.1.0`，并对 Gemini 当前及未来新增的 `finishReason` 做向前兼容。

在配置文件顶层设置 `whimsy = true` 可开启输入框星空，改为 `false` 关闭，运行中约 1 秒内
生效。也可在“设置 → 外观 → 输入框星空”切换，按 `Ctrl+S` 保存。开启时淡入 1 秒，随后
在输入框空白区域持续闪烁，打字、粘贴和已有草稿不会中断动画，也不会在 15 秒后结束。
关闭时用 75ms 淡出。终端失焦或打开弹窗时暂停重绘，返回输入框后继续显示。

## 使用文档

完整手册位于 [Docusaurus 文档站源码](website/docs/index.md)，包括安装、按键、配置、Provider、Gemini、
代理、日志、排错和开发参考。

本地启动文档站：

```shell
cd website
npm ci
npm run docs:dev
```

构建静态文档：

```shell
cd website
npm run docs:build
```

构建结果写入 `website/doc_build/`。

## License

MIT，见 [LICENSE](LICENSE)。
