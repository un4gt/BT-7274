# Titan

BT-7274 的独立 Ratatui 动画 crate。绘制源码取自 `E:\some_shit\titan\src\intro`，
保留机体、推进器、尘土、传感器与解析时间线，去掉独立演示程序的事件循环和重播按钮。
本 crate 不读写配置、终端或首次启动记录。

- `Intro::at(ms)` / `render(width, height, ms)`：3.5 秒降落、震屏、起身、点亮传感器。
- `render_startup(buffer, ms, panels, handoff)`：宿主先绘制不含机体的真实界面，再用
  `Handoff { area, idle }` 提供聊天内容区域及目标机体样式。2.75 秒开始以传感器为锚点，
  用五次缓动连续移动、缩放机体，聊天面板依次滑入，4.9 秒完成交接。
  `idle: None` 时机体随移动淡出；最终缓冲区与正常聊天界面一致。
- `Idle::new(background, armor, sensor)`：静态的已上线机体，适配主题与可用区域，没有周期刷新。

主应用负责每 16ms 调度开场帧、失焦暂停、输入隔离，以及完整播放后的持久化。
机体与震屏按窗口尺寸缩放，小窗口也播放完整时间线。

```sh
cargo test --workspace --locked
cargo test -p bt-7274 export_titan_preview -- --ignored --nocapture
python scripts/render_titan_preview.py
```

最后两条命令在 `target/titan-preview` 输出实际聊天界面的 60 FPS 动画 HTML 与 PNG 分镜，
覆盖桌面、80×24、浅色主题和关闭 idle 机体的场景。
