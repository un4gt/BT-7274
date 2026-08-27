# 日志

BT-7274 使用后台文件日志，并按本地日期自动切换文件。日志固定保存在用户主目录：

```text
~/.bt7274/logs/YYYY-MM-DD.log
```

Windows 对应：

```text
%USERPROFILE%\.bt7274\logs\YYYY-MM-DD.log
```

程序启动日志中的 `log_file` 字段会给出本次实际写入路径。

## 查看实时日志

PowerShell：

```powershell
Get-Content "$HOME\.bt7274\logs\$(Get-Date -Format yyyy-MM-dd).log" -Wait
```

macOS / Linux：

```shell
tail -f "$HOME/.bt7274/logs/$(date +%F).log"
```

## 调整日志级别

默认过滤器是 `bt_7274=info`。PowerShell 不能使用
`RUST_LOG=bt_7274=debug` 这种传统 Shell 写法，应先设置环境变量：

```powershell
$env:RUST_LOG = "bt_7274=debug"
bt-7274
```

只对当前 PowerShell 进程生效；关闭窗口后自动失效。

传统 Shell：

```shell
RUST_LOG=bt_7274=debug bt-7274
```

## 日志内容

日志包含应用版本、配置加载结果、Provider、协议、模型、代理模式、request id、请求耗时、
TTFT、usage、完成状态和经过分类的错误。程序不会主动记录聊天正文或完整 Provider 错误
响应，动态错误正文也不会直接显示在启动失败提示中。

API Key、Authorization、代理认证、自定义敏感 Header 和 URL 中的凭据会在诊断边界进行
脱敏。提交 Issue 时仍建议先人工检查日志，并只附上与问题相关的片段。

## 启动失败

终端只显示“诊断详情已安全写入日志”时，查看当天日志的最后几十行：

```powershell
Get-Content "$HOME\.bt7274\logs\$(Get-Date -Format yyyy-MM-dd).log" -Tail 80
```

结合 `stage`、文件位置和第一条 `ERROR` 判断是配置、会话、终端初始化还是网络阶段失败。
