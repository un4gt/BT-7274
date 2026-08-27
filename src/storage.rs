//! 小型私有数据文件的原子写入。
//!
//! 数据先写入目标目录中的唯一临时文件，完成 `flush` / `sync_all` 后再以
//! 平台原生的原子替换操作提交。这样进程在写入中途退出时，旧文件仍保持完整。

use color_eyre::eyre::{Context, ContextCompat, Result};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// 原子写入一个文件；Unix 新文件权限固定为 `0600`。
pub(crate) fn atomic_write_private(path: &Path, contents: &[u8]) -> Result<()> {
    atomic_write(path, contents)
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("原子写入目标缺少父目录")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建数据目录失败: {}", parent.display()))?;

    let temporary = temporary_path(path);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("创建临时文件失败: {}", temporary.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("设置临时文件权限失败: {}", temporary.display()))?;
        }

        file.write_all(contents)
            .with_context(|| format!("写入临时文件失败: {}", temporary.display()))?;
        file.flush()
            .with_context(|| format!("刷新临时文件失败: {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时文件失败: {}", temporary.display()))?;
        drop(file);

        replace_atomically(&temporary, path).with_context(|| {
            format!(
                "原子替换失败: {} -> {}",
                temporary.display(),
                path.display()
            )
        })?;

        #[cfg(unix)]
        if let Err(error) = std::fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            // 替换已经提交，不能再向调用方伪装成“未写入”；记录耐久性降级即可。
            tracing::warn!(
                path = ?parent,
                error = %error,
                "原子替换已完成，但同步父目录失败"
            );
        }

        Ok(())
    })();

    if result.is_err() {
        // 临时文件只属于本次写入；替换成功后它已不存在。
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data");
    path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4().simple()))
}

#[cfg(unix)]
fn replace_atomically(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
fn replace_atomically(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};

    const REPLACEFILE_WRITE_THROUGH: u32 = 0x0000_0001;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let target_exists = target.exists();
    let source = wide(source);
    let target = wide(target);
    let succeeded = unsafe {
        if target_exists {
            let replaced = ReplaceFileW(
                target.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if replaced != 0 {
                replaced
            } else {
                MoveFileExW(
                    source.as_ptr(),
                    target.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
        } else {
            MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn replace_atomically(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_complete_contents_and_leaves_no_temp_file() {
        let root =
            std::env::temp_dir().join(format!("bt-7274-atomic-write-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.json");

        atomic_write_private(&path, b"old").unwrap();
        atomic_write_private(&path, b"new-complete-value").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new-complete-value");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
