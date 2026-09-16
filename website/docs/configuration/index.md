# 配置文件

BT-7274 使用 TOML 配置。顶层 `whimsy` 支持运行时热更新，修改并保存后约 1 秒内生效；
其他字段在程序启动时读取，手动编辑后需要重启。
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
show_titan_on_startup = false
show_titan_when_idle = true
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
| `show_titan_on_startup` | 首次完整播放后，是否每次启动都播放动画；默认 `false` |
| `show_titan_when_idle` | 空对话且无草稿或活动任务时显示 BT；默认 `true` |
| `whimsy` | 输入框星空特效，默认 `false`，支持运行时开关 |
| `proxy` | 所有 Provider 请求共用的代理 |
| `context` | 可选的上下文预算与自动压缩设置 |
| `keybindings` | 可选的聊天编辑器快捷键 |
| `mcp_servers` | 以 Server 名称为键的远程 Streamable HTTP MCP 表 |

## BT 开场与空对话

首次启动会完整播放约 4.9 秒的降落、震屏、起身、点亮传感器和聊天面板展开动画，
不受 `show_titan_on_startup` 开关影响。完成后在系统数据目录的 `bt-7274/intro-completed`
记录一次；更新程序或修改配置不会重新触发首次开场。中途退出不记录完成，失焦时暂停计时。
后续启动仅在 `show_titan_on_startup = true` 时播放，可按 Esc 跳过；Ctrl+C 始终可退出。

机体上线后连续移动、缩放至聊天区域，采用两端减速的平滑曲线，聊天面板同步滑入。
关闭空对话机体显示时，机体在移动过程中逐渐淡出。

`show_titan_when_idle` 仅在无聊天历史、无草稿且未生成时显示静态 BT；输入空格或换行
也会隐藏，清空草稿后恢复。两项均可在“设置 → 外观”切换并保存。旧的 `show_titan`
作为 idle 开关继续读取，`show_titan_on_starup` 也接受为启动开关的拼写别名；保存时写入正式名称。

## 输入框星空

将 `whimsy = true` 写在第一个 TOML 表（例如 `[[providers]]`）之前即可启用，对所有模型生效。
也可在“设置 → 外观 → 输入框星空”切换，按 `Ctrl+S` 保存并立即生效。

启用后，在获得焦点的输入框空白区域持续闪烁：开启时用 1 秒淡入，关闭时用 75ms 淡出。
输入、粘贴、删除和移动光标均不会中断或重启动画，已有草稿时也会显示星空。星点各自以
4–7 秒的周期明暗变化，不设 15 秒结束时间。淡入淡出期间以 25ms 间隔重绘，常规闪烁
间隔为 150ms。文本、空格、选择区、边框和光标位置均保留。

终端失焦、切换到侧栏或打开弹窗时暂停星空重绘；返回输入框后沿原时间轴继续，无需重新开关。
将 `whimsy` 关闭后停止动画。编辑配置期间若 TOML 尚未写完整、`whimsy` 类型错误、文件
暂时不存在或版本不兼容，会保留上一次有效状态，待有效配置保存后继续更新。

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
