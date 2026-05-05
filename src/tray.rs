#[cfg(windows)]
pub fn spawn_daemon_tray_icon() {
    let _ = std::thread::Builder::new()
        .name("sheppard-tray".to_string())
        .spawn(|| {
            if let Err(error) = unsafe { run_tray_icon() } {
                eprintln!("Sheppard tray icon unavailable: {error}");
            }
        });
}

#[cfg(not(windows))]
pub fn spawn_daemon_tray_icon() {}

#[cfg(windows)]
unsafe fn run_tray_icon() -> Result<(), String> {
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
    use winapi::shared::windef::HWND;
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::shellapi::{
        Shell_NotifyIconW, NIF_ICON, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
        NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    };
    use winapi::um::winuser::{
        CreateWindowExW, DefWindowProcW, DestroyIcon, DispatchMessageW, GetMessageW, LoadIconW,
        PostQuitMessage, RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
        IDI_APPLICATION, MSG, WM_DESTROY, WNDCLASSW,
    };

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: UINT,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }

        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    let class_name = wide_null("SheppardTrayWindow");
    let window_name = wide_null("Sheppard daemon");
    let instance = GetModuleHandleW(null());

    let mut window_class: WNDCLASSW = zeroed();
    window_class.style = CS_HREDRAW | CS_VREDRAW;
    window_class.lpfnWndProc = Some(window_proc);
    window_class.hInstance = instance;
    window_class.lpszClassName = class_name.as_ptr();

    if RegisterClassW(&window_class) == 0 {
        return Err("could not register tray window class".to_string());
    }

    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        window_name.as_ptr(),
        0,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        instance,
        null_mut(),
    );

    if hwnd.is_null() {
        return Err("could not create tray window".to_string());
    }

    let icon = load_icon().unwrap_or_else(|| LoadedIcon {
        handle: LoadIconW(null_mut(), IDI_APPLICATION),
        owned: false,
    });
    if icon.handle.is_null() {
        return Err("could not load tray icon".to_string());
    }

    let mut notify_data: NOTIFYICONDATAW = zeroed();
    notify_data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    notify_data.hWnd = hwnd;
    notify_data.uID = 1;
    notify_data.uFlags = NIF_ICON | NIF_TIP;
    notify_data.hIcon = icon.handle;
    fill_wide_field(&mut notify_data.szTip, "Sheppard is active");

    if Shell_NotifyIconW(NIM_ADD, &mut notify_data) == 0 {
        if icon.owned {
            DestroyIcon(icon.handle);
        }
        return Err("could not add notification-area icon".to_string());
    }
    *notify_data.u.uVersion_mut() = NOTIFYICON_VERSION_4;
    Shell_NotifyIconW(NIM_SETVERSION, &mut notify_data);

    let mut message: MSG = zeroed();
    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }

    Shell_NotifyIconW(NIM_DELETE, &mut notify_data);
    if icon.owned {
        DestroyIcon(icon.handle);
    }
    Ok(())
}

#[cfg(windows)]
struct LoadedIcon {
    handle: winapi::shared::windef::HICON,
    owned: bool,
}

#[cfg(windows)]
unsafe fn load_icon() -> Option<LoadedIcon> {
    use std::ptr::null_mut;
    use winapi::um::winuser::{LoadImageW, IMAGE_ICON, LR_LOADFROMFILE};

    let icon_path = find_icon_path()?;
    let icon_path = wide_null(&icon_path.to_string_lossy());
    let icon = LoadImageW(
        null_mut(),
        icon_path.as_ptr(),
        IMAGE_ICON,
        16,
        16,
        LR_LOADFROMFILE,
    );

    if icon.is_null() {
        None
    } else {
        Some(LoadedIcon {
            handle: icon as winapi::shared::windef::HICON,
            owned: true,
        })
    }
}

#[cfg(windows)]
fn find_icon_path() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("assets").join("app_icon.ico"));
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("assets").join("app_icon.ico"));
            if let Some(parent) = exe_dir.parent() {
                candidates.push(parent.join("assets").join("app_icon.ico"));
                if let Some(grandparent) = parent.parent() {
                    candidates.push(grandparent.join("assets").join("app_icon.ico"));
                }
            }
        }
    }

    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn fill_wide_field<const N: usize>(field: &mut [u16; N], value: &str) {
    let wide = wide_null(value);
    for (target, source) in field.iter_mut().zip(wide.into_iter()) {
        *target = source;
    }
    field[N - 1] = 0;
}