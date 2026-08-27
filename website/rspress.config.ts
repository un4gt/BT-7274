import { defineConfig } from '@rspress/core';
import path from 'node:path';

export default defineConfig({
  root: path.join(__dirname, 'docs'),
  outDir: path.join(__dirname, 'doc_build'),
  lang: 'zh',
  title: 'BT-7274',
  description: '一个专注于对话体验的终端 AI Chat 客户端',
  icon: '/logo.svg',
  logo: '/logo.svg',
  logoText: 'BT-7274',
  globalStyles: path.join(__dirname, 'styles/index.css'),
  themeConfig: {
    darkMode: 'force-dark',
    nav: [
      {
        text: '开始使用',
        link: '/guide/getting-started',
      },
      {
        text: '配置',
        link: '/configuration/',
      },
      {
        text: 'Provider',
        items: [
          {
            text: 'Provider 管理',
            link: '/configuration/providers',
          },
          {
            text: 'Gemini',
            link: '/providers/gemini',
          },
          {
            text: '协议边界',
            link: '/provider-protocols',
          },
        ],
      },
      {
        text: '运维',
        items: [
          {
            text: '代理',
            link: '/network/proxy',
          },
          {
            text: '日志',
            link: '/operations/logging',
          },
          {
            text: '故障排查',
            link: '/operations/troubleshooting',
          },
        ],
      },
      {
        text: '开发参考',
        items: [
          { text: '架构', link: '/architecture' },
          { text: '上下文压缩', link: '/context-compaction' },
          { text: 'MCP', link: '/mcp' },
          { text: 'TUI 兼容性', link: '/tui-compatibility' },
          { text: '测试矩阵', link: '/test-matrix' },
        ],
      },
    ],
    sidebar: {
      '/': [
        {
          text: '开始使用',
          items: [
            { text: '安装与启动', link: '/guide/getting-started' },
            { text: '界面与按键', link: '/guide/interface-and-keys' },
          ],
        },
        {
          text: '配置',
          items: [
            { text: '配置文件', link: '/configuration/' },
            { text: 'Provider 管理', link: '/configuration/providers' },
            { text: 'Gemini', link: '/providers/gemini' },
            { text: '网络代理', link: '/network/proxy' },
          ],
        },
        {
          text: '运行与诊断',
          items: [
            { text: '日志', link: '/operations/logging' },
            { text: '故障排查', link: '/operations/troubleshooting' },
          ],
        },
        {
          text: '开发参考',
          items: [
            { text: '架构', link: '/architecture' },
            { text: 'Provider 协议', link: '/provider-protocols' },
            { text: '上下文压缩', link: '/context-compaction' },
            { text: '远程 MCP', link: '/mcp' },
            { text: 'TUI 兼容性', link: '/tui-compatibility' },
            { text: '测试矩阵', link: '/test-matrix' },
          ],
        },
      ],
    },
    socialLinks: [
      {
        icon: 'github',
        mode: 'link',
        content: 'https://github.com/un4gt/BT-7274',
      },
    ],
    editLink: {
      docRepoBaseUrl: 'https://github.com/un4gt/BT-7274/tree/main/website/docs',
      text: '在 GitHub 上编辑此页',
    },
    footer: {
      message:
        '<span>BT-7274 · Simple terminal chat, built with Rust and Ratatui.</span>',
    },
  },
});
