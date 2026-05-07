use crate::appdata::{AppConfig, AppPaths};
use crate::daemon;
use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use std::fs::OpenOptions;
use std::process::{Command, Stdio};

pub fn spawn_tray_process() -> Result<()> {
    let exe = std::env::current_exe().context("Failed to resolve current executable")?;

    let mut cmd = Command::new(exe);
    cmd.arg("tray")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let _child = cmd.spawn().context("Failed to spawn tray process")?;
    Ok(())
}

pub fn run_tray(paths: AppPaths, _cfg: AppConfig) -> Result<()> {
    // Keep the lock file alive for the lifetime of the tray process.
    let lock_path = paths.cache_dir.join("ytuff.tray.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("Failed to open tray lockfile: {lock_path:?}"))?;

    if lock_file.try_lock_exclusive().is_err() {
        // Another tray instance is already running.
        return Ok(());
    }

    // Ensure there's a daemon session to attach to.
    daemon::ensure_daemon(&paths)?;

    let exe = std::env::current_exe().context("Failed to resolve current executable")?;

    #[cfg(target_os = "linux")]
    {
        linux::run(exe)?;
        drop(lock_file);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        windows::run(exe)?;
        drop(lock_file);
        return Ok(());
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        drop(lock_file);
        Err(anyhow!("System tray is not supported on this OS"))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub fn run(_exe: std::path::PathBuf) -> Result<()> {
        Err(anyhow!("System tray is not supported on this GTK version"))
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::ffi::OsStr;
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, LoadIconW,
        PostQuitMessage, RegisterClassW, SetWindowLongPtrW, TranslateMessage, CREATESTRUCTW,
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HMENU, HWND_MESSAGE, IDI_APPLICATION,
        MSG, WM_APP, WM_CREATE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WNDCLASSW,
    };

    const WM_TRAYICON: u32 = WM_APP + 1;

    pub fn run(exe: std::path::PathBuf) -> Result<()> {
        let class_name = to_wstring("YTuffTray");

        unsafe {
            let hinstance = GetModuleHandleW(std::ptr::null());
            if hinstance.is_null() {
                return Err(anyhow!("GetModuleHandleW failed"));
            }

            let wnd_class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: class_name.as_ptr(),
                ..std::mem::zeroed()
            };

            if RegisterClassW(&wnd_class) == 0 {
                return Err(anyhow!("RegisterClassW failed"));
            }

            let exe_ptr = Box::into_raw(Box::new(exe)) as *mut std::ffi::c_void;

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                HWND_MESSAGE,
                0 as HMENU,
                hinstance,
                exe_ptr,
            );

            if hwnd.is_null() {
                let _ = Box::from_raw(exe_ptr as *mut std::path::PathBuf);
                return Err(anyhow!("CreateWindowExW failed"));
            }

            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAYICON;
            nid.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION as *const u16);
            set_tip(&mut nid, "YTuff");

            if Shell_NotifyIconW(NIM_ADD, &mut nid) == 0 {
                Shell_NotifyIconW(NIM_DELETE, &mut nid);
                DestroyWindow(hwnd);
                return Err(anyhow!("Shell_NotifyIconW(NIM_ADD) failed"));
            }

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, 0 as HWND, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            Shell_NotifyIconW(NIM_DELETE, &mut nid);
            DestroyWindow(hwnd);

            Ok(())
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let cs = lparam as *const CREATESTRUCTW;
                if !cs.is_null() {
                    let ptr = (*cs).lpCreateParams as isize;
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr);
                }
                0
            }
            WM_TRAYICON => {
                let event = lparam as u32;
                if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK {
                    let ptr = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                        hwnd,
                        GWLP_USERDATA,
                    ) as *const std::path::PathBuf;
                    if !ptr.is_null() {
                        let _ = spawn_restore_ui(&*ptr);
                    }
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn spawn_restore_ui(exe: &std::path::Path) -> Result<()> {
        let exe_str = exe.to_string_lossy().to_string();
        Command::new("cmd")
            .args(["/C", "start", "", &exe_str])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn terminal window")?;
        Ok(())
    }

    fn to_wstring(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(iter::once(0)).collect()
    }

    fn set_tip(nid: &mut NOTIFYICONDATAW, tip: &str) {
        let wide = to_wstring(tip);
        let max = nid.szTip.len();
        for i in 0..max {
            nid.szTip[i] = 0;
        }
        for (i, c) in wide.into_iter().take(max - 1).enumerate() {
            nid.szTip[i] = c;
        }
    }
}
