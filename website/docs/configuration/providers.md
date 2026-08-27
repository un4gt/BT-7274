# Provider 管理

Provider 定义协议、地址、认证方式与可用模型。可以在“设置 → 模型”中新增、编辑、删除、
同步模型，也可以直接编辑 TOML。

## 支持的协议

| `api_kind` | 默认 Base URL | 默认 API Key 环境变量 |
| --- | --- | --- |
| `chat_completions` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `responses` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `anthropic_messages` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| `gemini_generate_content` | `https://generativelanguage.googleapis.com/v1beta` | `GEMINI_API_KEY`，其次 `GOOGLE_API_KEY` |

## Provider 字段

```toml
[[providers]]
id = "provider-example"
name = "Example"
api_kind = "chat_completions"
base_url = "https://api.example.com/v1"
models = ["example-chat"]
api_key = "${EXAMPLE_API_KEY}"

[providers.headers]
x-tenant = "example"
```

| 字段 | 说明 |
| --- | --- |
| `id` | 会话引用 Provider 的稳定标识；不要随意重复或修改 |
| `name` | 设置页和 Footer 中显示的名称 |
| `api_kind` | 请求与流式响应所采用的原生协议 |
| `base_url` | 可选；省略时使用协议官方地址 |
| `api_key` | 可选；省略时读取协议默认环境变量 |
| `models` | 用户保留的模型列表；可同步后勾选，也可手动维护 |
| `headers` | 可选静态 Header；名称保存时统一为小写 |

`host`、`content-length`、`transfer-encoding` 和 `connection` 等传输层 Header 不允许覆盖。

## 编辑已有 Provider

1. 在侧栏按 `s` 打开设置。
2. 进入“模型”，选择 Provider。
3. 修改名称、协议、Base URL、API Key 或 Headers。
4. 同步模型目录，或直接编辑模型列表。
5. 保存并确认当前模型仍属于该 Provider。

如果调整了 Provider 顺序，TOML 中的 `current_provider` 也要指向正确下标。会话内部优先使用
稳定 `id` 解析 Provider，因此只修改展示名称不会破坏已有会话。

## 兼容服务

OpenAI 兼容服务应选择它实际实现的协议，而不是只根据品牌判断。只实现
`/chat/completions` 的服务应使用 `chat_completions`；只有实现了完整 Responses SSE 事件的
服务才应选择 `responses`。

协议事件与容错边界见 [Provider 协议边界](/provider-protocols)。
