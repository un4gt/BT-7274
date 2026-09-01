---
id: home
slug: /
title: BT-7274 文档
sidebar_label: 概览
description: BT-7274 终端 AI Chat 客户端的安装、配置、使用与开发文档。
---

BT-7274 是一个专注于对话体验的终端 AI Chat 客户端。它支持多 Provider、原生流式响应、
Markdown、会话持久化与全局代理，不包含代码代理或本地命令执行能力。

<img
  className="home-preview"
  src="/bt-7274-preview.svg"
  alt="BT-7274 终端聊天界面"
/>

## 从这里开始

- [安装与启动](/guide/getting-started)：安装发行包、配置 API Key 并完成第一次启动。
- [界面与按键](/guide/interface-and-keys)：了解输入、会话、设置及完整键盘操作。
- [配置文件](/configuration/)：查看配置路径、字段格式、环境变量和迁移规则。
- [Provider 管理](/configuration/providers)：新增、编辑 Provider 并维护模型列表。

## 协议与运行

BT-7274 分别处理 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 与
Gemini GenerateContent 协议。正文、reasoning、usage 和完成原因使用独立事件流；取消请求后，
已经生成的内容仍会保留。

- [Provider 协议边界](/provider-protocols)
- [Gemini 配置](/providers/gemini)
- [网络代理](/network/proxy)
- [日志与诊断](/operations/logging)

## 开发参考

架构、上下文压缩、远程 MCP、终端兼容性和测试范围位于左侧的“开发参考”分组。
