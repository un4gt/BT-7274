import { themes as prismThemes } from 'prism-react-renderer';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'BT-7274',
  tagline: '终端 AI Chat 客户端',
  favicon: 'logo.svg',
  url: 'https://un4gt.github.io',
  baseUrl: '/',
  organizationName: 'un4gt',
  projectName: 'BT-7274',
  staticDirectories: ['docs/public'],
  trailingSlash: false,
  onBrokenLinks: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },
  i18n: {
    defaultLocale: 'zh-Hans',
    locales: ['zh-Hans'],
    localeConfigs: {
      'zh-Hans': {
        label: '简体中文',
        htmlLang: 'zh-CN',
      },
    },
  },
  presets: [
    [
      'classic',
      {
        docs: {
          routeBasePath: '/',
          sidebarPath: './sidebars.js',
          editUrl: 'https://github.com/un4gt/BT-7274/edit/main/website/',
          showLastUpdateTime: true,
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      },
    ],
  ],
  themes: [
    [
      '@easyops-cn/docusaurus-search-local',
      {
        hashed: true,
        language: ['en', 'zh'],
        docsRouteBasePath: '/',
        indexBlog: false,
        highlightSearchTermsOnTargetPage: true,
        explicitSearchResultPath: true,
      },
    ],
  ],
  themeConfig: {
    image: 'bt-7274-preview.svg',
    colorMode: {
      defaultMode: 'light',
      disableSwitch: true,
      respectPrefersColorScheme: false,
    },
    docs: {
      sidebar: {
        hideable: true,
        autoCollapseCategories: false,
      },
    },
    navbar: {
      title: 'BT-7274',
      logo: {
        alt: 'BT-7274',
        src: 'logo.svg',
        width: 30,
        height: 30,
      },
      items: [
        {
          to: '/guide/getting-started',
          label: '开始使用',
          position: 'left',
        },
        {
          to: '/configuration/',
          label: '配置',
          position: 'left',
        },
        {
          label: 'Provider',
          position: 'left',
          items: [
            { label: 'Provider 管理', to: '/configuration/providers' },
            { label: 'Gemini', to: '/providers/gemini' },
            { label: '协议边界', to: '/provider-protocols' },
          ],
        },
        {
          label: '运维',
          position: 'left',
          items: [
            { label: '网络代理', to: '/network/proxy' },
            { label: '日志', to: '/operations/logging' },
            { label: '故障排查', to: '/operations/troubleshooting' },
          ],
        },
        {
          label: '开发参考',
          position: 'left',
          items: [
            { label: '架构', to: '/architecture' },
            { label: '上下文压缩', to: '/context-compaction' },
            { label: '远程 MCP', to: '/mcp' },
            { label: 'TUI 兼容性', to: '/tui-compatibility' },
            { label: '测试矩阵', to: '/test-matrix' },
          ],
        },
        {
          href: 'https://github.com/un4gt/BT-7274',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'light',
      copyright: `Copyright © ${new Date().getFullYear()} BT-7274`,
    },
    prism: {
      theme: prismThemes.github,
      additionalLanguages: ['bash', 'powershell', 'toml'],
    },
    tableOfContents: {
      minHeadingLevel: 2,
      maxHeadingLevel: 3,
    },
  },
};

export default config;
