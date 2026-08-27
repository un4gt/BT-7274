# 网络代理

全局代理用于 Provider 聊天请求、标题生成、模型同步，以及启用的远程 HTTP MCP 连接。
支持 HTTP/HTTPS 代理和 SOCKS5/SOCKS5H 代理。

## HTTP 代理

```toml
[proxy]
mode = "http"
url = "http://127.0.0.1:7890"
```

HTTPS 代理地址也使用 `http` 模式：

```toml
[proxy]
mode = "http"
url = "https://proxy.example.com:8443"
```

## SOCKS5 代理

```toml
[proxy]
mode = "socks5"
url = "socks5h://127.0.0.1:1080"
```

使用 `socks5h` 时，域名解析也交给代理；这通常更适合需要避免本地 DNS 解析的环境。

## 环境变量引用

```toml
[proxy]
mode = "http"
url = "${BT7274_PROXY_URL}"
```

PowerShell：

```powershell
$env:BT7274_PROXY_URL = "http://127.0.0.1:7890"
bt-7274
```

## 认证

代理认证可以直接放在 URL 中：

```toml
[proxy]
mode = "http"
url = "http://user:password@127.0.0.1:7890"
```

更推荐把完整 URL 放进环境变量，避免凭据直接写入配置文件。日志会对代理 URL 中的敏感
部分做脱敏。

## 校验规则

- `disabled` 模式不使用 `url`。
- `http` 只接受 `http://` 或 `https://`。
- `socks5` 只接受 `socks5://` 或 `socks5h://`。
- URL 必须包含 host，不能包含 query string 或 fragment。
- 配置中的环境变量引用可以在保存时暂时不存在，但实际发起请求时必须可解析。
