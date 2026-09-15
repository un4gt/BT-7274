# TUI 与终端兼容矩阵

## 自动化基线

自动化渲染使用 Ratatui `TestBackend`，覆盖 1x1、10x3、30x8、80x24 和
160x50。测试验证所有 overlay 不 panic、终端光标只归当前编辑层所有、三套
主题语义色对比度、Markdown 增量输入、代码横向裁剪、多行输入视口，以及
10,000 条消息只格式化当前视口附近的消息。

这类测试不等同于真实终端验收。CI 的 Windows、Linux 和 macOS runner 只证明
平台编译与逻辑测试通过，不能证明某个终端模拟器的键盘编码、OSC 52、resize
或 tmux/SSH 转发行为正确。

## 交互协议

| 能力 | 实现 | 兼容边界 |
| --- | --- | --- |
| 键盘与 resize | Crossterm event stream | 仅处理按键按下；缩放后重新计算布局与鼠标命中区域 |
| 鼠标 | Crossterm mouse capture | 点击过程摘要/步骤/标签、会话与设置入口；滚轮浏览；输入框定位与拖选；单纯移动不触发重绘。终端、tmux/SSH 需传递鼠标事件 |
| 多行粘贴 | Bracketed Paste | 应用启动时启用，退出或 panic unwind 时关闭 |
| 文本复制 | OSC 52，Base64 payload | 终端可能按自身策略禁用、询问或限制长度；应用上限 1 MiB |
| tmux 复制 | OSC 52 DCS passthrough | tmux 还需开启 `set-clipboard on` 或等价策略 |
| SSH 复制 | 由本地终端解释 OSC 52 | 中间 shell/tmux 必须保留控制序列 |
| 颜色 | 24-bit RGB 语义主题 | 不支持 truecolor 的终端由终端自身降级 |
| Unicode 宽度 | grapheme segmentation + `unicode-width` | 字形外观仍取决于终端字体和 emoji width 策略 |

## 人工 Smoke Matrix

“待测”表示没有可复核的真实终端证据，不能标记为通过。

| OS | Terminal | 版本 | 启动/退出 | 多行输入/粘贴 | CJK/emoji | resize | Markdown/代码 | OSC 52 | 状态 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Windows | Windows Terminal | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Windows | WezTerm | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Linux | Kitty | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Linux | Alacritty | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| Linux | Ghostty | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| macOS | iTerm2 | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| 任意 | tmux | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |
| 任意 | SSH | 待记录 | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 | 未执行 |

## Smoke 步骤

1. 记录日期、commit、OS、终端版本、shell、locale、字体和
   `COLORTERM`/`TERM`；使用 Release 二进制启动。
2. 在 30x8、80x24 和宽窗口间反复 resize，打开设置、模型、错误、压缩预览
   和 F4 代码查看器，确认无重叠、panic 或残留第二光标。
3. 输入中文、`👩‍💻`、`e` + U+0301、全角标点和超长单词；验证左右移动、
   Ctrl+Shift+方向键选择、Home/End、多行与历史。
4. 粘贴含 CRLF、Tab 和多行 Unicode 的文本，确认内容只进入当前编辑层且不会
   自动发送。
5. 流式输出 headings、bold、lists、table、blockquote、inline code 和未闭合
   fenced code；中途 resize 和取消。
6. 按 F4 查看长代码，验证上下/左右滚动、代码块切换与复制。tmux/SSH 场景还要
   在本地剪贴板读取结果。
7. 空闲至少 30 秒，确认界面不自行重绘；记录进程 CPU。随后启动生成或模型同步，
   确认 spinner 恢复并能取消。
8. 点击包含工具调用的过程摘要，切换参数/结果/原始页，在列表和详情上分别滚轮；窄屏
   验证“列表”和“关闭”。上翻消息后等待新输出，确认阅读位置不跳动。
9. 在中文、emoji、组合字符和自动换行的输入文本中点击与拖选，验证光标和选择边界；
   打开弹窗后点击原输入框位置，确认草稿不变。退出后确认终端的鼠标行为恢复。

人工记录格式：

```text
日期 / commit / OS / Terminal 版本 / shell / locale / font / 结果 / issue
```
