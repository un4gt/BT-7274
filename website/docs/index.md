---
pageType: home
title: BT-7274
titleSuffix: 终端 AI Chat 客户端

hero:
  badge:
    text: Simple TUI Chat · v0.1.0
    link: /guide/getting-started
  name: BT-7274
  text: 把 AI 对话留在终端里
  tagline: 一个专注、可配置、可诊断的终端聊天客户端。支持多 Provider、原生流式响应、Markdown、会话持久化与全局代理，不引入代码代理和本地命令执行。
  actions:
    - theme: brand
      text: 开始使用
      link: /guide/getting-started
    - theme: alt
      text: 查看配置
      link: /configuration/
  image:
    src: /hero-terminal.svg
    alt: BT-7274 终端聊天界面

features:
  - title: 四种原生协议
    details: OpenAI Chat Completions、Responses、Anthropic Messages 与 Gemini GenerateContent 由独立 Adapter 处理。
    link: /configuration/providers
  - title: 真正的流式对话
    details: 正文、reasoning、usage 与完成原因分别处理；取消请求后保留已经生成的部分回复。
    link: /provider-protocols
  - title: Provider 可维护
    details: 在设置中新增、编辑、删除 Provider，同步或手动更新模型列表，无需修改程序代码。
    link: /configuration/providers
  - title: 为终端而设计
    details: 响应式 TUI、Markdown 与代码块、多行编辑、键盘选择、OSC 52 复制和三套语义主题。
    link: /guide/interface-and-keys
  - title: 配置简单且可迁移
    details: TOML 配置支持环境变量引用；会话与日志按平台约定落盘，并对敏感诊断信息做脱敏。
    link: /configuration/
  - title: 可选远程 MCP
    details: 只连接远程 Streamable HTTP 服务，不启动 stdio 进程，也不会执行 MCP Tool。
    link: /mcp
---
