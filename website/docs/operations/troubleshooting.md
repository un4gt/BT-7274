# 故障排查

## 程序启动后立即退出

1. 打开当天的[日志文件](/operations/logging)。
2. 查找最早出现的 `ERROR`，不要只看终端中的简化提示。
3. 如果错误阶段是配置加载，检查 `config_version`、TOML 语法和 Provider 当前模型。
4. 如果存在 `config.toml.incompatible-*.bak`，说明配置结构版本不匹配；以当前
   `settings.example.toml` 为基础重新配置。

需要 Rust backtrace 时，PowerShell 使用：

```powershell
$env:RUST_BACKTRACE = "1"
bt-7274
```

## PowerShell 中 `RUST_LOG=...` 无效

PowerShell 的环境变量语法不同：

```powershell
$env:RUST_LOG = "bt_7274=debug"
bt-7274
```

## Provider 返回 401 或 403

- 确认 API Key 环境变量在启动 BT-7274 的同一个 Shell 中可见。
- 检查 `api_kind` 是否与服务真实协议一致。
- 自定义 Base URL 必须指向协议版本根目录，例如 OpenAI 常见的 `/v1`、Gemini 的
  `/v1beta`。
- 如果使用代理，先临时切换为 `disabled` 判断是否由代理认证或出口策略导致。

## Gemini 模型不存在

配置中的模型名必须是当前 API Key 可访问的 GenerateContent 模型。进入设置同步模型目录，
或根据 Google 控制台与官方文档手动更新 `models` 和顶层 `model`。

`gemini-...` 与 `models/gemini-...` 都可以输入；客户端会在构建端点时去掉 `models/` 前缀。

## 流式回复输出一部分后停止

先看日志中的完成原因：

- `MAX_TOKENS`：提高模型输出上限或缩短问题。
- 内容策略类原因：Provider 主动中止，已输出内容仍会保留。
- `Other(...)`：Provider 返回了非标准或新增完成原因，客户端会保留该原因而不是丢弃回复。
- 网络正文或 idle timeout：检查代理、连接稳定性和 Provider 状态。

Gemini 的未知 `finishReason` 已按 `Other` 向前兼容，不会再因为新增枚举值将已输出回复整体
变为反序列化错误。

## 代理无法连接

- `http` 模式只使用 `http://` 或 `https://` URL。
- `socks5` 模式只使用 `socks5://` 或 `socks5h://` URL。
- 代理 URL 不能带 query 或 fragment。
- 环境变量引用必须在运行 BT-7274 的进程中存在。

完整示例见[网络代理](/network/proxy)。

## Markdown、颜色或字符宽度异常

优先在 Windows Terminal、WezTerm、Kitty 或其他支持 truecolor 与现代 Unicode 的终端中
测试，并选择覆盖 CJK 与常用符号的等宽字体。tmux、SSH 与 OSC 52 的限制见
[TUI 与终端兼容矩阵](/tui-compatibility)。

## 复制没有进入系统剪贴板

BT-7274 使用 OSC 52。终端可能禁用、询问或限制该控制序列；tmux 还需要启用
`set-clipboard on` 或等价配置。可以先在本地终端直连运行，排除 tmux/SSH 转发问题。
