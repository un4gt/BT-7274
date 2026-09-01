/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docs: [
    'home',
    {
      type: 'category',
      label: '开始使用',
      collapsed: false,
      items: ['guide/getting-started', 'guide/interface-and-keys'],
    },
    {
      type: 'category',
      label: '配置',
      collapsed: false,
      items: [
        'configuration/index',
        'configuration/providers',
        'providers/gemini',
        'network/proxy',
      ],
    },
    {
      type: 'category',
      label: '运行与诊断',
      collapsed: false,
      items: ['operations/logging', 'operations/troubleshooting'],
    },
    {
      type: 'category',
      label: '开发参考',
      collapsed: false,
      items: [
        'architecture',
        'provider-protocols',
        'context-compaction',
        'mcp',
        'tui-compatibility',
        'test-matrix',
      ],
    },
  ],
};

export default sidebars;
