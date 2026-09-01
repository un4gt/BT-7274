# 配置文件

BT-7274 使用 TOML 配置。程序启动时读取一次配置，不监听文件变化；手动编辑后需要重启。
在设置 Overlay 中按 `Ctrl+S` 保存时，程序会校验并重写整个配置文件，因此手写注释、字段
顺序和未知字段不会保留。

## 文件位置

| 内容 | Windows | Linux | macOS |
| --- | --- | --- | --- |
| 配置 | `%APPDATA%\bt-7274\config.toml` | `${XDG_CONFIG_HOME:-~/.config}/bt-7274/config.toml` | `~/Library/Application Support/bt-7274/config.toml` |
| 会话 | `%APPDATA%\bt-7274\sessions\*.json` | `${XDG_DATA_HOME:-~/.local/share}/bt-7274/sessions/*.json` | `~/Library/Application Support/bt-7274/sessions/*.json` |
| 日志 | `%USERPROFILE%\.bt7274\logs\YYYY-MM-DD.log` | `~/.bt7274/logs/YYYY-MM-DD.log` | `~/.bt7274/logs/YYYY-MM-DD.log` |

## 最小 Gemini 配置

```toml
config_version = 8
current_provider = 0
model = "gemini-2.5-flash"
language = "zh"
theme = "vanguard"
show_titan = false

[[providers]]
id = "provider-gemini"
name = "Gemini"
api_kind = "gemini_generate_content"
models = ["gemini-2.5-flash"]
api_key = "${GEMINI_API_KEY}"

[proxy]
mode = "disabled"
```

官方 Gemini Base URL 与 API Key 环境变量都有默认值，因此不需要写入 `base_url`。更多说明
见 [Gemini Provider](/providers/gemini)。

## 顶层字段

| 字段 | 说明 |
| --- | --- |
| `config_version` | 必须等于当前程序要求的版本；当前为 `8` |
| `providers` | Provider 数组，至少保留一个 |
| `current_provider` | 当前 Provider 在数组中的下标，从 `0` 开始 |
| `model` | 当前模型，必须属于当前 Provider 的模型列表 |
| `language` | `zh` 或 `en` |
| `theme` | `vanguard`、`carbon` 或 `paper` |
| `show_titan` | 是否显示聊天区像素画背景 |
| `proxy` | 所有 Provider 请求共用的代理 |
| `context` | 可选的上下文预算与自动压缩设置 |
| `keybindings` | 可选的聊天编辑器快捷键 |
| `mcp_servers` | 以 Server 名称为键的远程 Streamable HTTP MCP 表 |

## 可选默认项

下面两段不写时，程序使用相同的默认值：

```toml
[context]
auto_compact = true
auto_compact_threshold_percent = 85
reserved_output_tokens = 4096

[keybindings]
submit = "enter"
newline = "ctrl+o"
history_previous = "ctrl+p"
history_next = "ctrl+n"
word_left = "ctrl+left"
word_right = "ctrl+right"
select_all = "ctrl+a"
copy = "alt+c"
```

面向简单聊天客户端的配置不会持久化模型能力快照和逐模型参数表；这些运行时信息由程序
内部维护。

## 环境变量引用

API Key 与代理 URL 可以使用完整的 `${ENV_VAR}` 形式：

```toml
api_key = "${GEMINI_API_KEY}"
```

引用只在运行时解析，配置文件仍保存变量名。推荐用这种方式避免凭据直接落盘。

MCP 遵循 Codex 风格：`bearer_token_env_var` 和 `env_http_headers` 直接填写环境变量名，
不使用 `${...}` 包装。详见 [MCP 远程连接](/mcp)。

## 配置版本

项目不迁移旧结构。`config_version` 缺失或不匹配时，原文件会备份为
`config.toml.incompatible-<id>.bak`，随后使用当前默认配置启动。修正配置后重启即可。
