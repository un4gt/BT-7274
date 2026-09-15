use crate::app::App;
use crate::config::Settings;
use crate::runtime::conversation::Session;

mod app;
mod clipboard;
mod config;
mod editor;
mod event;
mod i18n;
mod logging;
mod runtime;
mod secret;
mod session;
mod storage;
mod text;
mod ui;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableBracketedPaste,
            crossterm::event::DisableFocusChange,
            crossterm::event::DisableMouseCapture
        );
        ratatui::restore();
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let log_guard = logging::init()?;
    logging::install_redacted_panic_hook();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        log_file = %log_guard.current_file().display(),
        "应用启动"
    );

    let result = run_app().await;
    match result {
        Ok(()) => {
            tracing::info!("应用正常退出");
            Ok(())
        }
        // 启动错误可能来自含 API Key 的损坏配置文件；退出边界不记录动态正文。
        Err(_err) => {
            tracing::error!("应用异常退出（错误链正文已省略）");
            Err(color_eyre::eyre::eyre!(logging::PUBLIC_FAILURE_MESSAGE))
        }
    }
}

async fn run_app() -> color_eyre::Result<()> {
    let settings = Settings::load().inspect_err(|_| {
        tracing::error!(stage = "settings", "应用启动阶段失败（动态错误正文已省略）");
    })?;
    let sessions = Session::load_all().inspect_err(|_| {
        tracing::error!(stage = "sessions", "应用启动阶段失败（动态错误正文已省略）");
    })?;
    tracing::info!(
        providers = settings.providers.len(),
        sessions = sessions.len(),
        "应用数据加载完成"
    );
    let mut terminal = ratatui::init();
    let _terminal_guard = TerminalGuard;
    // ratatui 0.30 的 init() 不再清屏；部分 Windows 终端的备用屏幕
    // 初始并非空白，会残留进入前的 shell 输出，这里显式清一次
    terminal.clear().inspect_err(|_| {
        tracing::error!(stage = "terminal_clear", "终端初始化阶段失败");
    })?;
    // 开启括号粘贴：终端把一次粘贴作为单一 Paste 事件发给应用，
    // 而不是退化成逐字符按键（避免粘贴内容里的 j/k 等被当作导航键）
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableFocusChange,
        crossterm::event::EnableMouseCapture
    )
    .inspect_err(|_| {
        tracing::error!(stage = "bracketed_paste", "终端初始化阶段失败");
    })?;
    App::new(settings, sessions).run(terminal).await
}
