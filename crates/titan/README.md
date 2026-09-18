# Titan

BT-7274 的独立 Ratatui 动画 crate。绘制源码取自 `E:\some_shit\titan\src\intro`，
保留机体、推进器、尘土、传感器与解析时间线，去掉独立演示程序的事件循环和重播按钮。
本 crate 不读写配置、终端或首次启动记录。

- `Intro::at(ms)` / `render(width, height, ms)`：3.5 秒降落、震屏、起身、点亮传感器。
- `render_startup(buffer, ms, panels, handoff)`：宿主先绘制不含机体的真实界面，再用
  `Handoff { area, idle }` 提供聊天内容区域及目标机体样式。2.75 秒开始以传感器为锚点，
  用五次缓动连续移动、缩放机体，3.65 秒完成交接。面板从第一帧即位于最终位置，
  边框约半秒内渐显，内容 0.65 秒内就绪；不对字符边框做逐格平移。
  `idle: None` 时机体随移动淡出；最终缓冲区与正常聊天界面一致。
- `Idle::new(background, armor, sensor)`：静态的已上线机体，适配主题与可用区域，没有周期刷新。
  `render_with_opacity(area, buffer, opacity)` 支持由宿主调度淡入淡出，零透明度不修改缓冲区。
- `Idle::animate(IdleAnimation::at(ms))`：依次巡视、握拳校准、竖拇指，初次等待 5 秒，
  每个动作 2.4 秒，间隔 12 秒。`IdleAnimation::next_frame_in()` 在动作中返回帧间隔，
  静止时直接返回到下一动作的等待时间。宿主负责失焦、遮挡与输入时冻结时钟。
  三种尺寸均独立配置头部、肩肘和手指造型，双脚固定，关节使用缓动和亚字符覆盖率。

主应用负责每 16ms 调度开场帧、失焦暂停、输入隔离，以及完整播放后的持久化。
机体与震屏按窗口尺寸缩放，小窗口也播放完整时间线。

```sh
cargo test --workspace --locked
cargo test -p bt-7274 export_titan_preview -- --ignored --nocapture
cargo test -p bt-7274 export_idle_fade_preview -- --ignored --nocapture
cargo test -p bt-7274 export_idle_actions_preview -- --ignored --nocapture
python scripts/render_titan_preview.py
```

导出测试及 Python 脚本在 `target/titan-preview` 输出实际聊天界面的 60 FPS 动画 HTML 与 PNG 分镜，
覆盖桌面、80×24、浅色主题、关闭 idle 机体，以及输入和清空时的淡入淡出。
`idle-actions*.html` 展示三种待机动作，预览省略了动作之间的长等待。
