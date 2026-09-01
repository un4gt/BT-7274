# Gemini Provider

BT-7274 直接实现 Gemini GenerateContent REST 协议，不通过 OpenAI 兼容层。聊天使用
`streamGenerateContent?alt=sse`，会话标题使用 `generateContent`，模型同步使用 Gemini 模型
目录接口。

## 最小配置

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

`base_url` 可省略，默认是：

```text
https://generativelanguage.googleapis.com/v1beta
```

模型名可以写成 `gemini-2.5-flash` 或 `models/gemini-2.5-flash`，请求前会统一处理。示例模型
仅用于展示配置格式；建议在设置中同步模型目录后选择账号实际可用的模型。

## 设置 API Key

PowerShell：

```powershell
$env:GEMINI_API_KEY = "你的 API Key"
bt-7274
```

传统 Shell：

```shell
export GEMINI_API_KEY="你的 API Key"
bt-7274
```

如果没有 `GEMINI_API_KEY`，程序会继续尝试 `GOOGLE_API_KEY`。

## 自动请求头

Gemini 请求会自动携带：

```text
x-goog-api-key: <已解析的 API Key>
x-goog-api-client: bt-7274/0.1.0
```

API Key Header 会标记为敏感字段，不写入诊断日志。`x-goog-api-client` 由程序版本生成，用户
不需要在 `[providers.headers]` 中重复配置。

## 流式响应兼容

解析器接受 Gemini 当前 SSE 分块中的增量 `content.parts`、`thought`、`thoughtSignature`、
`usageMetadata`、`modelVersion`、`responseId` 和服务等级元数据。聊天正文只呈现普通 `text`；
`thought = true` 的文本进入 reasoning 区域；函数调用等非聊天 Part 不会执行，也不会混入
正文。

当前公开的 `finishReason` 会映射为统一完成状态：

| Gemini 原因 | BT-7274 状态 |
| --- | --- |
| `STOP` | 正常完成 |
| `MAX_TOKENS` | 达到输出上限 |
| `SAFETY`、`RECITATION`、`LANGUAGE`、`BLOCKLIST`、`PROHIBITED_CONTENT`、`SPII` | 内容策略中止 |
| `IMAGE_SAFETY`、`IMAGE_PROHIBITED_CONTENT`、`IMAGE_RECITATION` | 内容策略中止 |
| `MALFORMED_FUNCTION_CALL`、`UNEXPECTED_TOOL_CALL`、`TOO_MANY_TOOL_CALLS` | 保留原因为 `Other` |
| `FINISH_REASON_UNSPECIFIED`、`OTHER`、`NO_IMAGE`、`IMAGE_OTHER` | 保留原因为 `Other` |

Google 将来增加新的枚举值时，BT-7274 会降级为 `Other(<原值>)`，不会因为已经输出一段内容
后遇到未知 `finishReason` 而把整次回复标记为读取失败。

## 官方参考

- [Gemini API：Generating content](https://ai.google.dev/api/generate-content)
- [Gemini API 文档](https://ai.google.dev/gemini-api/docs)
