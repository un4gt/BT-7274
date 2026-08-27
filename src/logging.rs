//! 适用于 TUI 的文件日志：后台写入，并按本地日期滚动。

use chrono::{Local, NaiveDate};
use color_eyre::eyre::{Context, ContextCompat, Result, eyre};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

const DEFAULT_FILTER: &str = "bt_7274=info";
pub const PUBLIC_FAILURE_MESSAGE: &str =
    "应用启动或运行失败；诊断详情已安全写入日志，敏感错误正文已省略";

/// 保持后台日志线程存活，并在退出时等待缓冲区写入完成。
pub struct LogGuard {
    _worker_guard: WorkerGuard,
    directory: PathBuf,
}

impl LogGuard {
    /// 当前本地日期对应的日志文件。
    pub fn current_file(&self) -> PathBuf {
        dated_log_path(&self.directory, Local::now().date_naive())
    }
}

/// 初始化全局文件日志。日志级别可通过标准的 `RUST_LOG` 调整。
pub fn init() -> Result<LogGuard> {
    let directory = log_dir()?;
    prepare_log_dir(&directory)?;
    let appender = DailyFileAppender::new(directory.clone())
        .with_context(|| format!("初始化日志文件失败: {}", directory.display()))?;
    let (writer, worker_guard) = NonBlockingBuilder::default().lossy(false).finish(appender);
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_timer(LocalTimer)
        .with_ansi(false)
        .with_target(true)
        .compact()
        .try_init()
        .map_err(|err| eyre!("初始化日志订阅器失败: {err}"))?;

    Ok(LogGuard {
        _worker_guard: worker_guard,
        directory,
    })
}

/// Replace the default panic reporter because panic payloads can contain a
/// failed request or configuration value. Location is useful for diagnostics;
/// payload and captured environment are deliberately omitted.
pub fn install_redacted_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(location) = info.location() {
            tracing::error!(
                file = location.file(),
                line = location.line(),
                column = location.column(),
                "应用发生 panic（payload 已省略）"
            );
        } else {
            tracing::error!("应用发生 panic（payload 与位置均已省略）");
        }
        eprintln!("{PUBLIC_FAILURE_MESSAGE}");
    }));
}

/// 固定使用用户主目录，保证各平台路径均为 `~/.bt7274/logs`。
pub fn log_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("无法定位用户主目录")?
        .join(".bt7274")
        .join("logs"))
}

fn prepare_log_dir(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("创建日志目录失败: {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("设置日志目录权限失败: {}", directory.display()))?;
    }
    Ok(())
}

fn dated_log_path(directory: &Path, date: NaiveDate) -> PathBuf {
    directory.join(format!("{}.log", date.format("%Y-%m-%d")))
}

fn open_log_file(directory: &Path, date: NaiveDate) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(dated_log_path(directory, date))
}

struct DailyFileAppender {
    directory: PathBuf,
    date: NaiveDate,
    file: File,
}

impl DailyFileAppender {
    fn new(directory: PathBuf) -> io::Result<Self> {
        let date = Local::now().date_naive();
        let file = open_log_file(&directory, date)?;
        Ok(Self {
            directory,
            date,
            file,
        })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let date = Local::now().date_naive();
        if date != self.date {
            self.file.flush()?;
            self.file = open_log_file(&self.directory, date)?;
            self.date = date;
        }
        Ok(())
    }
}

impl Write for DailyFileAppender {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.rotate_if_needed()?;
        self.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        write!(
            writer,
            "{}",
            Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_name_uses_iso_local_date() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 23).unwrap();
        assert_eq!(
            dated_log_path(Path::new("logs"), date),
            Path::new("logs").join("2026-08-23.log")
        );
    }

    #[test]
    fn appender_creates_and_appends_to_dated_file() {
        let directory = std::env::temp_dir().join(format!(
            "bt-7274-log-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        prepare_log_dir(&directory).unwrap();
        let mut appender = DailyFileAppender::new(directory.clone()).unwrap();
        appender.write_all(b"first\n").unwrap();
        appender.write_all(b"second\n").unwrap();
        appender.flush().unwrap();
        let path = dated_log_path(&directory, appender.date);
        drop(appender);

        assert_eq!(std::fs::read_to_string(path).unwrap(), "first\nsecond\n");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn public_crash_message_contains_no_dynamic_payload() {
        let secret = "panic-secret-value";
        assert!(!PUBLIC_FAILURE_MESSAGE.contains(secret));
        assert!(!PUBLIC_FAILURE_MESSAGE.contains("payload="));
    }
}
