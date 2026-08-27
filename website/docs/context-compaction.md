# 上下文预算与压缩

## 预算

BT-7274 的输入预算只包含两层：

- `System`：客户端明确生成的 system instructions 与系统事件。
- `Conversation`：压缩摘要、用户正文、助手正文和可见 reasoning。

`Reserved Output` 单独计入 planned tokens，不属于输入层。

当前 Provider 没有统一 tokenizer，因此程序使用保守估算：ASCII 连续字符约每 4 个计一个
token，CJK、emoji 和非空白标点按单个 token 计。界面以 `~` 标注估算值。

当模型目录或配置提供 `context_window` 时：

```text
planned = estimated input + reserved output
overflow = max(planned - context window, 0)
```

确定溢出会在网络请求前被阻止。

## 手动压缩

输入 `/compact` 后先显示：

- 压缩前后的预计输入 token。
- 将归档的消息数。
- 提取式摘要预览。

按 Enter 才提交，Esc 取消。若旧消息不足以形成安全压缩边界，则不创建空压缩记录。

## 自动压缩

启用 `auto_compact` 后，当 planned tokens 达到
`auto_compact_threshold_percent` 时，在发送新请求前使用同一压缩流程。自动压缩会在聊天
区显示提示，不静默丢弃历史。

## 摘要

摘要按以下稳定分区保存：

- Decisions
- Constraints
- Code state
- TODO
- Key references
- Other retained context

这是提取式摘要，不额外调用模型。摘要属于 Conversation 数据，四种 Provider Adapter 都将
它编码为首条 user 级消息，不能提升为 system instructions。

## 撤销

每次提交保存：

- 压缩 id 与时间。
- 是否自动触发。
- 上一次摘要。
- 本次归档的完整消息。

输入 `/undo-compact` 恢复最近一次记录中的消息和摘要。撤销后再次原子保存会话。

## 生成中消息

压缩不会归档 `streaming` 消息，并保留最近四条消息。归档边界会向前调整到用户消息，避免
把一轮对话拆成不完整的半轮。

## 回归要求

- 中英文长对话的关键约束、决定、TODO 和引用仍进入摘要。
- 最近对话保持原文。
- 生成中消息不会被归档。
- apply 后执行 undo 能恢复原始消息序列。
- 摘要 token 只进入 Conversation 层。
