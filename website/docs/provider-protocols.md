# Provider 协议边界

## 统一契约

每个 Adapter 实现三项能力：

1. 流式生成聊天回复。
2. 生成会话标题。
3. 获取模型目录与可用能力元数据。

Adapter 由 Provider 的 `api_kind` 决定，而不是由模型名称猜测。例如，经 OpenRouter 一类
Gateway 使用 Gemini 模型但调用 OpenAI Chat Completions 端点时，应配置
`api_kind = "chat_completions"`，工具也会使用 OpenAI 格式；只有 Gateway 暴露 Gemini
GenerateContent 端点时才选择 `gemini_generate_content`。

统一流事件包含 assistant text、reasoning、system、tool call、tool result、completed、error
和 cancelled。Provider 原生 payload 必须先映射为该契约，不能把 reasoning、工具结果或系
统状态拼进正文。

普通历史只包含 user/assistant 对话正文。本轮 MCP 循环会精确编码工具定义、assistant 工具
调用和工具结果；完成后的工具轨迹作为结构化会话片段保存，但不会在以后轮次重复发送。

## OpenAI Chat Completions

- 端点：`POST /chat/completions`
- `stream = true`，请求 `stream_options.include_usage`
- 正文读取 `delta.content`
- 兼容 reasoning 字段读取 `delta.reasoning_content` / `delta.reasoning`
- 工具使用 OpenAI `tools[].function`；按 `index` 重组流式 `delta.tool_calls` 参数片段
- 后续请求追加 assistant `tool_calls` 和 `role = "tool"` 结果
- 要求合法 JSON、已知 `finish_reason` 和最终 `[DONE]`
- `stop`、`length`、`content_filter` 映射为统一完成原因
- Provider 返回非聊天 finish reason 时保留为 `Other` 并在 UI 提示

## OpenAI Responses

- 端点：`POST /responses`
- 使用 `async-openai` 的强类型 Response/Stream 事件校验
- 正文读取 `response.output_text.delta`
- reasoning summary/text delta 映射为 reasoning
- `response.queued` 映射为系统事件
- `response.completed` 提供 usage 和完成边界
- 工具使用 Responses function tool；解析 output item 与 function-call arguments 事件
- 后续输入追加 `function_call` 和 `function_call_output`
- 工具请求默认包含 `reasoning.encrypted_content`，并原样回传 reasoning/output items

只登记两条兼容规则：

- `responses.empty_truncation`：将明确的 `"truncation":""` 归一化为 `disabled`
- `opencode.responses.ping`：忽略明确的 `{"type":"ping"}` 心跳

其他未知事件继续报 Protocol Error。日志不记录响应正文。

## Anthropic Messages

- 端点：`POST /messages`
- 鉴权使用 `x-api-key` 与 `anthropic-version`
- `text_delta` 映射正文，`thinking_delta` 映射 reasoning
- `message_start` 与 `message_delta` 汇总 input/cache/output usage
- 要求最终 `message_stop` 和 stop reason
- 按 Anthropic 版本策略跳过未来未知事件，只记录有长度限制的事件类型
- 当前不向 Anthropic Messages 暴露 MCP 工具

## Gemini GenerateContent

- 端点：`POST /models/{model}:streamGenerateContent?alt=sse`
- 普通 text part 映射正文，`thought = true` 的 text part 映射 reasoning
- 工具声明使用 `functionDeclarations`，响应解析 `functionCall`
- 后续请求使用 model `functionCall` 和 user `functionResponse`
- Gemini 3 返回的 model parts（包括 `thoughtSignature`）在工具下一轮原样回传
- `usageMetadata` 映射 token usage
- 已知 safety/limit finish reason 映射统一完成原因
- 未知 finish reason 报 Protocol Error

## 模型能力

`ModelCapabilities` 使用 `supported`、`unsupported`、`unknown` 三态：

- text
- vision
- audio
- reasoning
- structured output
- streaming
- context window
- max output tokens

请求需要的能力被明确标记为 `unsupported` 时，在本地拒绝；`unknown` 允许尝试。已知
context window 和 max output 会进入请求前预算校验。

## 模型参数

通用参数：

- `temperature`
- `top_p`
- `max_output_tokens`
- 整体、首包、流空闲超时
- 最大重试次数

reasoning 参数保持协议原生表示：OpenAI effort、Anthropic thinking budget 和 Gemini
thinking budget 不互相转换。

`extra_body` 只能补充非结构顶层字段。模型、消息、输入、stream、instructions、鉴权及会
改变请求类别的字段不能被覆盖。

## 网络与重试

- 连接失败以及 408/409/429/500/502/503/504 可进入有限退避。
- 首包超时不自动重试，避免重复计费。
- 尊重 `Retry-After`，单次等待最多 30 秒，总尝试最多 5 次。
- quota、billing、insufficient quota/credit balance 不重试。
- 成功响应开始读取后，断流、协议错误和 idle timeout 直接失败并保留部分回复。
- 请求、SSE 读取和退避都监听同一 Cancellation Token。

## 持久化

每轮消息保存实际 `provider_id`、Provider 名称快照、模型名与请求参数。助手消息保存
completed/streaming/cancelled/failed 状态、脱敏错误摘要，以及独立的 MCP 调用/结果片段。

压缩摘要作为首条 user 级 Conversation 消息编码，不进入任何协议的 system/instructions。

## Fixtures

- `tests/fixtures/openai_chat.sse`
- `tests/fixtures/openai_responses.sse`
- `tests/fixtures/anthropic_messages.sse`
- `tests/fixtures/gemini_generate_content.sse`

Fixtures 和单元 payload 覆盖文本、usage、reasoning/心跳、工具调用、完成边界和流提前结
束。新增兼容规则必须先增加可复现的脱敏 fixture。
