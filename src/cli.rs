//! 不启动聊天界面的命令行操作。

use std::{
    io::{self, Write},
    path::Path,
};

use clap::{ArgMatches, Command};
use color_eyre::eyre::{Context, Result, bail};

use crate::config::Settings;

mod open;

pub(crate) fn command() -> Command {
    Command::new(env!("CARGO_PKG_NAME"))
        .bin_name(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about("终端 AI 聊天客户端")
        .after_help("不带参数时启动交互式聊天界面。")
        .subcommand(config_command())
}

fn config_command() -> Command {
    Command::new("config")
        .about("查看或编辑配置文件")
        .after_help("配置文件不存在时，show 和 edit 会先创建默认配置。")
        .subcommand(Command::new("show").about("打印配置文件路径和原始内容"))
        .subcommand(Command::new("edit").about("使用系统默认应用打开配置文件"))
}

pub(crate) fn run_config(args: &ArgMatches) -> Result<()> {
    match args.subcommand() {
        Some(("show", _)) => show_config(&Settings::config_path()?, &mut io::stdout().lock()),
        Some(("edit", _)) => edit_config(&Settings::config_path()?, open::file),
        None => {
            config_command()
                .bin_name(concat!(env!("CARGO_PKG_NAME"), " config"))
                .print_help()?;
            println!();
            Ok(())
        }
        _ => unreachable!("clap rejects unknown config subcommands"),
    }
}

fn ensure_config_file(path: &Path) -> Result<()> {
    // 不调用 Settings::load：即使版本过旧或 TOML 损坏，也要保留原文件供用户修复。
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => bail!("配置路径不是文件: {}", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Settings::default().save_to(path),
        Err(error) => Err(error).with_context(|| format!("检查配置文件失败: {}", path.display())),
    }
}

fn show_config(path: &Path, output: &mut impl Write) -> Result<()> {
    ensure_config_file(path)?;
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读取配置失败: {}", path.display()))?;
    writeln!(output, "配置文件: {}\n", path.display())?;
    output.write_all(raw.as_bytes())?;
    Ok(())
}

fn edit_config(path: &Path, open_file: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    ensure_config_file(path)?;
    open_file(path).with_context(|| format!("打开配置文件失败: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("bt-7274-config-cli-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> PathBuf {
            self.0.join("中文配置 & spaces").join("config.toml")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn show_preserves_original_contents_even_for_incompatible_or_broken_toml() {
        let fixture = Fixture::new();
        let path = fixture.path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for raw in [
            "# 保留注释和 CRLF\r\nconfig_version = 0\r\napi_key = \"${TEST_KEY}\"\r\n",
            "# 编辑中的配置\n[unfinished",
            "",
        ] {
            std::fs::write(&path, raw).unwrap();
            let mut output = Vec::new();

            show_config(&path, &mut output).unwrap();

            assert_eq!(
                String::from_utf8(output).unwrap(),
                format!("配置文件: {}\n\n{raw}", path.display())
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
            assert_eq!(
                std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
                1
            );
        }
    }

    #[test]
    fn show_creates_a_valid_default_config_when_missing() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let mut output = Vec::new();

        show_config(&path, &mut output).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            toml::from_str::<Settings>(&raw).unwrap(),
            Settings::default()
        );
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("配置文件: {}\n\n{raw}", path.display())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn edit_creates_missing_config_before_opening_and_preserves_existing_files() {
        let fixture = Fixture::new();
        let path = fixture.path();
        edit_config(&path, |opened| {
            assert_eq!(opened, path);
            let raw = std::fs::read_to_string(opened)?;
            assert_eq!(
                toml::from_str::<Settings>(&raw).unwrap(),
                Settings::default()
            );
            Ok(())
        })
        .unwrap();

        // 编辑入口甚至应允许修复非 UTF-8 的文件。
        let raw = b"# broken config\n[unfinished\xff";
        std::fs::write(&path, raw).unwrap();
        edit_config(&path, |opened| {
            assert_eq!(opened, path);
            assert_eq!(std::fs::read(opened)?, raw);
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn invalid_config_path_does_not_launch_an_application() {
        let fixture = Fixture::new();
        let path = fixture.path();
        std::fs::create_dir_all(&path).unwrap();

        assert!(show_config(&path, &mut Vec::new()).is_err());
        assert!(edit_config(&path, |_| panic!("must not open a directory")).is_err());
    }

    #[test]
    fn edit_reports_launch_errors_with_the_config_path() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let error = edit_config(&path, |_| {
            Err(io::Error::from(io::ErrorKind::NotFound).into())
        })
        .unwrap_err();

        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        assert!(path.is_file());
    }
}
