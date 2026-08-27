# 安装与启动

BT-7274 是一个单进程 TUI Chat 客户端。正常使用只需要可执行文件、一个可用的 Provider
API Key，以及支持现代终端控制序列的终端模拟器。

## 安装

发布包由 cargo-dist 构建，覆盖 Windows、macOS 与 Linux。

### Windows PowerShell

```powershell
irm https://github.com/un4gt/BT-7274/releases/latest/download/bt-7274-installer.ps1 | iex
```

### macOS / Linux

```shell
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/un4gt/BT-7274/releases/latest/download/bt-7274-installer.sh | sh
```

### 从源码运行

需要当前稳定版 Rust：

```shell
git clone https://github.com/un4gt/BT-7274.git
cd BT-7274
cargo run --release
```

也可以直接安装到 Cargo 的 bin 目录：

```shell
cargo install --git https://github.com/un4gt/BT-7274 --locked
```

## 第一次启动

程序首次启动时会使用 OpenAI Chat Completions 与 `gpt-4o-mini` 作为默认配置。先在当前
Shell 中提供 API Key，再启动程序即可。

PowerShell：

```powershell
$env:OPENAI_API_KEY = "你的 API Key"
bt-7274
```

传统 Shell：

```shell
export OPENAI_API_KEY="你的 API Key"
bt-7274
```

如果要使用 Gemini，在侧栏按 `s` 打开设置，新建协议为
`gemini_generate_content` 的 Provider；也可以直接使用
[Gemini 配置示例](/providers/gemini)。

## 最小操作流程

1. 在输入框输入消息，按 `Enter` 发送。
2. 按 `Ctrl+O` 插入换行。
3. 生成过程中按 `Esc` 或 `Ctrl+C` 取消；已经生成的内容会保留。
4. 按 `Tab` 在输入区与侧栏之间切换。
5. 在侧栏按 `n` 新建会话，按 `s` 打开设置。

完整按键见[界面与按键](/guide/interface-and-keys)。

## 启动检查

首次配置后建议完成一次最小检查：

- Provider 能同步或手动保存至少一个模型。
- 发送简单问题后能看到增量输出和最终完成状态。
- 退出再启动后，会话仍能从本地恢复。
- 网络失败时，日志目录中生成了当天的日志文件。

如果程序直接退出，请先查看[日志](/operations/logging)和[故障排查](/operations/troubleshooting)。
