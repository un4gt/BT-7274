//! 事件系统：终端事件（crossterm）与应用事件（聊天流、任务结果）
//! 汇入同一条 mpsc 通道，由主循环统一消费。

use color_eyre::eyre::OptionExt;
use crossterm::event::Event as CrosstermEvent;
use futures::{FutureExt, StreamExt};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

use crate::config::WhimsyWatcher;
use crate::runtime::mcp::McpRuntimeEvent;
use crate::runtime::model::{ModelCatalog, ModelStreamEvent};

/// The frequency at which tick events are emitted.
const TICK_FPS: f64 = 10.0;

/// Representation of all possible events.
#[derive(Clone, Debug)]
pub enum Event {
    /// An event that is emitted on a regular schedule.
    ///
    /// Use this event to run any code which has to be updated with being a direct response to a
    /// user event. e.g. polling a server, updating an animation, or rendering the UI based on a
    /// fixed frame rate.
    Tick,
    /// Crossterm events.
    ///
    /// These are emitted by the terminal.
    Crossterm(CrosstermEvent),
    /// Application events.
    ///
    /// Use this to emit custom events that are specific to your application.
    App(AppEvent),
    /// 终端输入流失败或意外结束。
    Error(String),
}

/// Application events.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// 退出应用。
    Quit,
    /// 配置文件中 whimsy 的有效值发生变化。
    WhimsyChanged(bool),
    /// Model Runtime 的统一流事件；`stream` 用于丢弃取消后迟到的事件。
    ModelStream {
        stream: u64,
        event: ModelStreamEvent,
    },
    /// MCP Server 连接状态或能力元数据发生变化。
    McpRuntime(McpRuntimeEvent),
    /// 标题生成完成。
    ///
    /// `session` 为目标会话 id：生成期间可能已切换会话，须按 id 定位。
    TitleGenerated { session: String, title: String },
    /// 模型列表同步完成（向导或设置主界面触发）。
    ///
    /// `task` 为同步任务代号，用于区分来源并丢弃过期结果。
    ModelsSynced {
        task: u64,
        models: Result<ModelCatalog, String>,
    },
}

/// Terminal event handler.
#[derive(Debug)]
pub struct EventHandler {
    /// Event sender channel.
    sender: mpsc::UnboundedSender<Event>,
    /// Event receiver channel.
    receiver: mpsc::UnboundedReceiver<Event>,
    /// 仅在界面存在可见动画或需要周期检查点时启用 tick。
    animation: watch::Sender<bool>,
}

impl EventHandler {
    /// Constructs a new instance of [`EventHandler`] and spawns a new thread to handle events.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (animation, animation_receiver) = watch::channel(false);
        let actor = EventTask::new(sender.clone(), animation_receiver);
        tokio::spawn(async { actor.run().await });
        Self {
            sender,
            receiver,
            animation,
        }
    }

    /// 通道发送端克隆，供流式生成等后台任务推送事件。
    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.sender.clone()
    }

    /// 低频检查配置；文件读取放在线程池，未变化时不唤醒 TUI 重绘。
    /// 仅在实际运行主循环时启动，避免测试或离屏渲染读取用户配置。
    pub fn watch_whimsy(&self, path: PathBuf) {
        let sender = self.sender();
        tokio::spawn(async move {
            let mut watcher = WhimsyWatcher::new(path);
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = sender.closed() => break,
                    _ = interval.tick() => {}
                }
                let Ok((updated, value)) = tokio::task::spawn_blocking(move || {
                    let value = watcher.poll();
                    (watcher, value)
                })
                .await
                else {
                    break;
                };
                watcher = updated;
                if let Some(value) = value
                    && sender
                        .send(Event::App(AppEvent::WhimsyChanged(value)))
                        .is_err()
                {
                    break;
                }
            }
        });
    }

    /// Receives an event from the sender.
    ///
    /// This function blocks until an event is received.
    ///
    /// # Errors
    ///
    /// This function returns an error if the sender channel is disconnected. This can happen if an
    /// error occurs in the event thread. In practice, this should not happen unless there is a
    /// problem with the underlying terminal.
    pub async fn next(&mut self) -> color_eyre::Result<Event> {
        self.receiver
            .recv()
            .await
            .ok_or_eyre("Failed to receive event")
    }

    /// Queue an app event to be sent to the event receiver.
    ///
    /// This is useful for sending events to the event handler which will be processed by the next
    /// iteration of the application's event loop.
    pub fn send(&mut self, app_event: AppEvent) {
        // Ignore the result as the reciever cannot be dropped while this struct still has a
        // reference to it
        let _ = self.sender.send(Event::App(app_event));
    }

    /// 开关周期 tick。关闭时事件任务只等待真实终端或应用事件。
    pub fn set_animation_enabled(&self, enabled: bool) {
        self.animation.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    #[cfg(test)]
    pub fn animation_enabled(&self) -> bool {
        *self.animation.borrow()
    }
}

/// A task that handles crossterm events and emits ticks only while animation is enabled.
struct EventTask {
    /// Event sender channel.
    sender: mpsc::UnboundedSender<Event>,
    animation: watch::Receiver<bool>,
}

impl EventTask {
    /// Constructs a new instance of [`EventTask`].
    fn new(sender: mpsc::UnboundedSender<Event>, animation: watch::Receiver<bool>) -> Self {
        Self { sender, animation }
    }

    /// Runs the event task.
    ///
    /// Disabled ticks have no timer wake-up; terminal and application events still arrive normally.
    async fn run(mut self) -> color_eyre::Result<()> {
        let tick_rate = Duration::from_secs_f64(1.0 / TICK_FPS);
        let mut reader = crossterm::event::EventStream::new();
        let mut tick = tokio::time::interval(tick_rate);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let animation_enabled = *self.animation.borrow_and_update();
            let crossterm_event = reader.next().fuse();
            tokio::select! {
              _ = self.sender.closed() => {
                break;
              }
              changed = self.animation.changed() => {
                if changed.is_err() {
                    break;
                }
                if *self.animation.borrow() {
                    tick.reset();
                }
              }
              _ = tick.tick(), if animation_enabled => {
                self.send(Event::Tick);
              }
              event = crossterm_event => {
                match event {
                    Some(Ok(crossterm::event::Event::Mouse(crossterm::event::MouseEvent {
                        kind: crossterm::event::MouseEventKind::Moved,
                        ..
                    }))) => {}
                    Some(Ok(event)) => self.send(Event::Crossterm(event)),
                    Some(Err(err)) => {
                        self.send(Event::Error(format!("terminal event stream failed: {err}")));
                        break;
                    }
                    None => {
                        self.send(Event::Error("terminal event stream ended unexpectedly".to_owned()));
                        break;
                    }
                }
              }
            };
        }
        Ok(())
    }

    /// Sends an event to the receiver.
    fn send(&self, event: Event) {
        // Ignores the result because shutting down the app drops the receiver, which causes the send
        // operation to fail. This is expected behavior and should not panic.
        let _ = self.sender.send(event);
    }
}
