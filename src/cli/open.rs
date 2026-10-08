//! 通过系统文件关联打开配置，路径直接传递给系统 API 或进程参数。

use std::path::Path;

use color_eyre::eyre::Result;

#[cfg(windows)]
pub(super) fn file(path: &Path) -> Result<()> {
    use std::{io, os::windows::ffi::OsStrExt, ptr};

    use color_eyre::eyre::bail;
    use windows_sys::Win32::{
        Foundation::ERROR_CANCELLED,
        System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        },
        UI::{
            Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };

    // Shell 扩展可能依赖 COM；该命令在主线程、Tokio runtime 初始化前执行。
    let initialized = unsafe {
        CoInitializeEx(
            ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 {
        bail!("初始化系统文件打开服务失败 (HRESULT: 0x{initialized:08X})");
    }
    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            // SAFETY: 与本线程成功的 CoInitializeEx 调用配对。
            unsafe { CoUninitialize() };
        }
    }
    let _com = ComGuard;

    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        // 等待系统完成启动交接，避免 CLI 退出时取消异步打开操作。
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        // 默认动词通常为 open；未关联应用时由 Shell 显示可保存默认应用的选择器。
        // 即使设置 NO_UI，lpVerb 为 NULL 仍允许系统的“打开方式”流程。
        lpVerb: ptr::null(),
        lpFile: wide_path.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    // SAFETY: 结构大小正确，所有字符串均以 NUL 结尾且在调用期间保持有效。
    if unsafe { ShellExecuteExW(&mut info) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
        return Ok(());
    }
    Err(error.into())
}

#[cfg(not(windows))]
pub(super) fn file(path: &Path) -> Result<()> {
    use color_eyre::eyre::{Context, bail};

    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(opener)
        .arg(path)
        .status()
        .with_context(|| format!("启动系统文件打开命令失败: {opener}"))?;
    if !status.success() {
        bail!("系统文件打开命令 {opener} 执行失败: {status}");
    }
    Ok(())
}
