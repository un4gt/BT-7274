# BT-7274 TODO

> 产品定位：可靠、克制的 TUI Chat 客户端。
>
> MCP 边界：仅支持远程 Streamable HTTP；服务端响应可使用 JSON 或 SSE。
>
> 最后更新：2026-08-26。

## 当前进度

| 大项 | 完成度 | 当前状态 |
| --- | ---: | --- |
| 范围收缩 | 9 / 9 | 已完成并通过完整门禁 |
| 聊天基线 | 16 / 16 | 已完成并有单元、协议与 TUI 测试 |
| P0 发布门禁 | 0 / 5 | 自动化基础已具备，等待真实环境验收 |
| P1 聊天体验 | 0 / 9 | 尚未进入实现阶段 |
| P1 配置与运维 | 0 / 5 | 尚未进入实现阶段 |
| 最终自动化验证 | 5 / 5 | 当前 Debug 132 passed；最近 Release 129 passed |

当前阶段应优先完成 P0 的真实终端、Provider、远程 MCP、崩溃恢复和性能证据；在这些
发布门禁完成前，不提前推进 P1。

## 使用约定

- `[x]` 表示已有实现与测试证据。
- `[ ]` 表示尚未完成，不以“可配置”代替真实协议验证。
- 每完成一个大项，只集中执行一次该大项测试；通过后再更新标记。
- 配置结构只接受当前版本，不增加旧版本迁移或兼容分支。

## 已完成：范围收缩

- [x] 删除自主执行循环、工具注册与审批策略。
- [x] 删除本地目录读写、搜索、代码补丁和命令执行能力。
- [x] 删除本地 MCP 进程启动、子进程管理和远程工具调用。
- [x] 删除执行步骤持久化及其聊天区、详情和设置 UI。
- [x] 将会话结构收缩为正文、reasoning、系统事件和生成终态。
- [x] 将上下文预算收缩为 System、Conversation 和 Reserved Output。
- [x] Provider 请求不再发送能力 schema，也不编码结构化调用历史。
- [x] MCP Transport 仅保留 Streamable HTTP，并拒绝其他类型配置。
- [x] 删除超出 TUI Chat 产品边界的扩展路线。

> 完成记录：2026-08-25；Rustfmt、Clippy、Debug/Release 测试全部通过，Debug/Release
> 均为 129 passed / 0 failed。

## 已完成：聊天基线

- [x] Provider 与 Model 分离，一个 Provider 可维护多个模型。
- [x] Provider 支持新增、编辑、删除、手动添加模型和模型目录同步。
- [x] 支持 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 和 Gemini GenerateContent。
- [x] 支持自定义 Base URL、API Key、静态 Headers 和 per-model 参数。
- [x] 支持 HTTP/HTTPS、SOCKS5/SOCKS5H 全局代理。
- [x] 支持文本与 reasoning 流式输出，Esc/Ctrl+C 中止并保留部分回复。
- [x] 支持应用默认、会话默认和下一轮临时模型选择。
- [x] 支持会话新建、重命名、搜索、删除和自动标题。
- [x] 支持 completed、streaming、cancelled、failed 消息状态和崩溃恢复。
- [x] 支持 Unicode 感知的上下文估算、溢出阻止和可撤销压缩。
- [x] 支持 Markdown、代码查看器、OSC 52 复制和长历史视口渲染。
- [x] 支持多行编辑、历史、按词移动、选择和快捷键配置。
- [x] 支持中英双语、三套主题和可选像素画背景。
- [x] 支持按日期日志、`RUST_LOG` 过滤和已知凭据脱敏。
- [x] 支持远程 MCP 连接的新增、编辑、启停、重连、状态与能力摘要。
- [x] 设置弹窗的单行控件严格裁剪在所属区域内，分类选中背景不会越过行或弹窗边界。

> 当前 Debug 自动化基线：132 个测试覆盖配置、会话、上下文、四种 Provider 协议、SSE、取消、
> 远程 MCP mock、日志脱敏、编辑器、Markdown、主题和窄终端渲染。

## P0：发布门禁

- [ ] **P0-001 完成终端兼容矩阵**：Windows Terminal、WezTerm、Kitty、Alacritty、Ghostty、iTerm2、tmux 和 SSH 均记录 smoke 结果。
- [ ] **P0-002 完成真实 Provider smoke**：四种协议各验证正文、reasoning、usage、取消、限流、超时和异常流。
- [ ] **P0-003 完成远程 MCP smoke**：使用真实 Streamable HTTP Server 验证 JSON/SSE 初始化、鉴权、代理、重连和退出清理。
- [ ] **P0-004 完成跨平台崩溃恢复**：验证流式检查点、损坏会话隔离和强制退出后的三种恢复选择。
- [ ] **P0-005 建立发布性能基线**：冷启动、空闲 CPU、输入延迟、长会话滚动、内存和退出耗时有固定数据集。

### P0 已有证据

| 项目 | 已有自动化证据 | 仍缺少的验收 |
| --- | --- | --- |
| P0-001 | 1x1/窄终端、三主题、弹窗单光标与长历史渲染测试 | 八种真实终端 smoke 记录 |
| P0-002 | 四协议 fixtures、reasoning、usage、错误、心跳和断流测试 | 四种真实 Provider 的完整 smoke |
| P0-003 | 本地 Streamable HTTP mock 的 JSON initialize、能力摘要和关闭测试 | 真实服务的 JSON/SSE、鉴权、代理和重连 |
| P0-004 | 损坏会话隔离、streaming 状态及 Continue/Keep/Discard 单元测试 | 跨平台强制退出与磁盘恢复 |
| P0-005 | 长历史视口渲染、按需 tick 和取消状态测试 | 固定机器、固定数据集的量化结果 |

## P1：聊天体验

- [ ] **P1-010 增加会话导入导出**：支持 Markdown 与完整 JSON，默认不导出凭据或内部诊断详情。
- [ ] **P1-011 增加会话分支**：从任意轮次创建新会话并保存来源关系，不修改原会话。
- [ ] **P1-012 增加上下文查看器**：展示 System、Conversation、Reserved Output 和压缩摘要占用。
- [ ] **P1-013 完善 token 与费用统计**：按轮次、会话、模型和 Provider 汇总；未知价格不猜测。
- [ ] **P1-014 增加日志查看器**：筛选级别、复制当天日志路径并查看最近错误。
- [ ] **P1-015 增加图片输入**：MIME/大小校验、模型能力校验、预览信息和各 Provider 原生编码。
- [ ] **P1-016 增加通用附件输入**：明确支持的类型、解析方式、大小限制和上下文成本。
- [ ] **P1-017 增加结构化输出**：按 Provider 能力发送 JSON Schema，并展示校验错误。
- [ ] **P1-018 完善 reasoning 设置**：按协议展示原生参数，并区分可见 reasoning 与仅 usage 可见的推理消耗。

## P1：配置与运维

- [ ] **P1-030 接入系统密钥存储**：配置只保存引用，支持更新、删除和失效诊断。
- [ ] **P1-031 增加配置来源查看**：展示默认值、配置文件和临时环境变量的最终生效结果。
- [ ] **P1-032 增加诊断包**：导出版本、配置摘要、终端信息、request id 和最近脱敏日志，不包含聊天正文。
- [ ] **P1-033 完善 MCP 连接详情**：展示 URL 摘要、协议版本、服务端版本、能力、最近错误和重连时间。
- [ ] **P1-034 增加 MCP 自动重连策略**：仅连接级故障退避重连，支持取消且不制造忙循环。

### P1 已有基础

- P1-012：Footer 已展示上下文总量、输出预留和模型窗口，但还没有分层查看器。
- P1-013：单次请求已有 usage、输出速度和日志指标，但还没有会话汇总与费用模型。
- P1-018：已有协议原生 reasoning 参数和独立 reasoning 流事件，但还没有完整设置 UI 与
  reasoning token 分项。
- P1-030：已有环境变量引用与脱敏，但没有接入系统密钥存储。
- P1-033：已有连接状态、协议/服务端版本、能力和最近错误，仍缺 URL 摘要与重连时间。
- P1-034：已有手动连接/重连及连接故障隔离，尚无自动退避重连。

## 最终验证

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --locked --all-targets -- -D warnings`
- [x] `cargo test --locked --all-targets`：132 passed / 0 failed
- [x] `cargo test --release --locked --all-targets`：129 passed / 0 failed
- [x] 扫描源码与产品文档，确认没有已删除模块、配置字段或 UI 文案残留。

> 验证记录：2026-08-25；Rustfmt 通过，Clippy 零警告，Debug/Release 测试全部通过。
> 本次启动恢复修复按“大项一次测试”约定仅重跑 Debug；Release 的最近基线仍为
> 129 passed / 0 failed。
