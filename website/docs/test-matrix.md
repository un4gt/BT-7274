# BT-7274 测试矩阵

## 批次原则

- 每完成 `TODO.md` 的一个大项，集中执行一次本地验证。
- CI 对主分支持续执行完整门禁，不因本地批次策略降低标准。
- Provider 与 MCP 使用脱敏 fixture 或本地 mock；CI 不依赖真实 API Key。
- 自动化不能替代真实终端与真实服务 smoke。

## 自动化门禁

| 门禁 | 命令 | 目的 |
| --- | --- | --- |
| Format | `cargo fmt --all -- --check` | 稳定格式 |
| Clippy | `cargo clippy --locked --all-targets -- -D warnings` | 零警告和常见缺陷 |
| Debug | `cargo test --locked --all-targets` | 主测试集与平台差异 |
| Release | `cargo test --release --locked --all-targets` | 优化配置差异 |
| Docs | `cargo doc --no-deps` | 文档构建 |

## 单元与协议测试

- 当前配置版本、非法字段、Provider/Model、Proxy、Headers 和环境变量引用。
- 文本宽度、折行、截断、多行编辑、选择、历史和快捷键冲突。
- 消息终态、模型参数快照、损坏会话隔离和恢复选择。
- Unicode token 估算、预算溢出、压缩摘要、apply/undo 和生成中消息保护。
- Cancellation Token、Esc/Ctrl+C 优先级、request id、TTFT 与输出 token 指标。
- 四种 Provider 的正文、reasoning、usage、完成原因、错误和提前断流。
- Chat Completions、Responses、Gemini 的工具定义编码、流式工具调用解析与结果回送。
- SSE 注释、空行、拆包、心跳、非法 JSON 和连接中断。
- MCP Streamable HTTP initialize、`tools/list`、`tools/call`、能力摘要、配置重连、脱敏、取消
  和有界关闭。

## 持久化测试

- 配置只接受当前 `config_version`；旧版、未来版和缺失版本均不迁移，原样备份后使用默认配置启动。
- 临时文件完成写入和同步后原子替换，目录中不遗留临时文件。
- 单个损坏会话不影响其他会话加载。
- streaming 消息重启后可继续、保留或丢弃。
- 会话 JSON roundtrip 保留 Provider/Model/参数、reasoning、系统事件、工具片段和终态。

## TUI 渲染测试

- 1x1、窄、标准和超宽尺寸不 panic。
- 所有弹窗最多设置一个终端光标，关闭弹窗后焦点正确恢复。
- 三套主题具备可读对比度，焦点和状态不只依赖颜色。
- Markdown headings、bold、lists、tables、blockquote、inline/fenced code 稳定渲染。
- 未闭合 Markdown 在增量流期间不造成布局跳变或 panic。
- 长历史只格式化当前视口附近消息，空闲时不产生周期 tick。
- 消息区溢出时显示滚动条；空流式回复显示动画；工具结果默认折叠且可展开。
- MCP 列表和向导在窄终端可滚动，凭据字段始终遮蔽。

## 故障注入

| 场景 | 预期 |
| --- | --- |
| DNS/连接失败 | 对应请求或 MCP Server 失败，TUI 继续运行 |
| 首包/整体/流空闲超时 | 显示具体阶段，可取消，不泄露正文 |
| HTTP 401/403 | Authentication，不输出 API Key |
| HTTP 429 + Retry-After | 可取消退避，显示限流分类 |
| HTTP 5xx | 按有限策略退避 |
| 异常 SSE/未知事件 | Protocol Error；仅忽略登记过的心跳 |
| 流中途断开 | 保留部分回复并标记 failed |
| 会话 JSON 截断 | 隔离损坏文件 |
| MCP initialize 超时 | 仅对应 Server 标记 failed |
| MCP tools/list 失败 | 对应 Server 标记 failed，不暴露半成品工具目录 |
| MCP tools/call 失败 | 错误结果回送模型，其他 Server 与 TUI 继续运行 |
| 模型反复调用工具 | 8 轮或单轮 16 调用上限触发 Protocol Error |
| MCP 连接意外关闭 | 状态变为 failed，可手动重连 |
| MCP 配置变更 | 旧 generation 状态不能覆盖新连接 |
| 应用退出 | 恢复终端并有界关闭网络任务 |

## 人工终端矩阵

详细步骤见 [tui-compatibility.md](tui-compatibility.md)。

| OS | Terminal | 启动/退出 | 输入/粘贴 | CJK/emoji | resize | 流式/取消 | 状态 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Windows | Windows Terminal | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Windows | WezTerm | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Linux | Kitty | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Linux | Alacritty | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Linux | Ghostty | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| macOS | iTerm2 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| 任意 | tmux | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| 任意 | SSH | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |

记录格式：`日期 / commit / OS / Terminal 版本 / shell / locale / font / 结果 / issue`。

## 性能预算

| 指标 | 初始目标 |
| --- | ---: |
| 冷启动到首帧 | ≤ 2 秒 |
| 空闲 CPU | 稳定低于 1% |
| 普通按键到下一帧 | ≤ 50 ms |
| 10,000 条消息滚动 | 无秒级卡顿 |
| 取消到状态更新 | ≤ 250 ms，不含远端关闭 |
| 正常退出清理 | ≤ 8 秒，MCP 总关闭上限 |

## 大项记录模板

```text
大项：...
日期：YYYY-MM-DD
修改范围：...
本地测试：...
结果：...
CI：链接或待运行
人工 smoke：...
遗留风险：...
TODO 标记：仅在验证通过后更新
```
