#[cfg(windows)]
pub fn spawn_daemon_tray_icon() {
    if std::env::var("SHEPHERD_DAEMON_TRAY").ok().as_deref() == Some("0") {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("sheppard-tray".to_string())
        .spawn(|| {
            if let Err(error) = unsafe { run_tray_icon(TrayMode::Daemon) } {
                eprintln!("Sheppard tray icon unavailable: {error}");
            }
        });
}

#[cfg(not(windows))]
pub fn spawn_daemon_tray_icon() {}

#[cfg(windows)]
pub struct TrayController {
    exit_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(windows)]
impl TrayController {
    pub fn exit_requested(&self) -> bool {
        self.exit_requested
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(not(windows))]
pub struct TrayController;

#[cfg(not(windows))]
impl TrayController {
    pub fn exit_requested(&self) -> bool {
        false
    }
}

#[cfg(windows)]
pub fn spawn_tui_tray_controller() -> TrayController {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use winapi::um::wincon::GetConsoleWindow;

    let exit_requested = Arc::new(AtomicBool::new(false));
    let console_hwnd = unsafe { GetConsoleWindow() } as isize;

    // Apply the workspace app icon to the console window so the taskbar
    // displays the proper Sheppard icon (not the generic console glyph).
    unsafe {
        if console_hwnd != 0 {
            apply_console_icon(console_hwnd as winapi::shared::windef::HWND);
        }
    }

    let state = Arc::new(TuiTrayState {
        console_hwnd,
        hidden: AtomicBool::new(false),
        exit_requested: Arc::clone(&exit_requested),
    });
    let _ = TUI_TRAY_STATE.set(Arc::clone(&state));

    let monitor_state = Arc::clone(&state);
    let _ = std::thread::Builder::new()
        .name("sheppard-tui-minimize-watch".to_string())
        .spawn(move || unsafe { watch_tui_minimize(monitor_state) });

    let _ = std::thread::Builder::new()
        .name("sheppard-tui-tray".to_string())
        .spawn(|| {
            if let Err(error) = unsafe { run_tray_icon(TrayMode::Tui) } {
                eprintln!("Sheppard TUI tray icon unavailable: {error}");
            }
        });

    TrayController { exit_requested }
}

#[cfg(not(windows))]
pub fn spawn_tui_tray_controller() -> TrayController {
    TrayController
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayMode {
    Daemon = 1,
    Tui = 2,
}

#[cfg(windows)]
struct TuiTrayState {
    console_hwnd: isize,
    hidden: std::sync::atomic::AtomicBool,
    exit_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(windows)]
static TRAY_MODE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
static TUI_TRAY_STATE: std::sync::OnceLock<std::sync::Arc<TuiTrayState>> =
    std::sync::OnceLock::new();

#[cfg(windows)]
const WM_TRAYICON: u32 = 0x8000 + 1;

#[cfg(windows)]
const TRAY_CMD_OPEN: i32 = 1001;

#[cfg(windows)]
const TRAY_CMD_EXIT: i32 = 1002;

#[cfg(windows)]
unsafe fn apply_console_icon(hwnd: winapi::shared::windef::HWND) {
    use winapi::um::winuser::{SendMessageW, ICON_BIG, ICON_SMALL, WM_SETICON};

    let big = load_icon_at_size(32, 32)
        .map(|i| i.handle)
        .unwrap_or(std::ptr::null_mut());
    let small = load_icon_at_size(16, 16)
        .map(|i| i.handle)
        .unwrap_or(std::ptr::null_mut());

    if !big.is_null() {
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
    }
    if !small.is_null() {
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
    }
}

#[cfg(windows)]
unsafe fn run_tray_icon(mode: TrayMode) -> Result<(), String> {
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
    use winapi::shared::windef::HWND;
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::shellapi::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
        NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    };
    use winapi::um::winuser::{
        CreateWindowExW, DefWindowProcW, DestroyIcon, DispatchMessageW, GetMessageW, LoadIconW,
        PostQuitMessage, RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, IDI_APPLICATION,
        MSG, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
        WNDCLASSW,
    };

    TRAY_MODE.store(mode as u32, std::sync::atomic::Ordering::SeqCst);

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: UINT,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_TRAYICON {
            // NOTIFYICON_VERSION_4 callback layout:
            //   LOWORD(lparam) = mouse/notification message
            //   HIWORD(lparam) = icon ID (we use 1)
            //   LOWORD(wparam) = anchor x, HIWORD(wparam) = anchor y
            let event = (lparam as usize & 0xFFFF) as u32;
            match event {
                WM_LBUTTONDBLCLK | WM_LBUTTONUP => open_from_tray(),
                WM_CONTEXTMENU | WM_RBUTTONUP => {
                    let lo = (wparam as usize & 0xFFFF) as u16 as i16 as i32;
                    let hi = ((wparam as usize >> 16) & 0xFFFF) as u16 as i16 as i32;
                    show_tray_menu(hwnd, lo, hi);
                }
                _ => {}
            }
            return 0;
        }

        if msg == WM_COMMAND {
            let command = (wparam as usize & 0xFFFF) as i32;
            match command {
                TRAY_CMD_OPEN => open_from_tray(),
                TRAY_CMD_EXIT => request_exit_from_tray(),
                _ => {}
            }
            return 0;
        }

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

    let icon = load_icon_at_size(16, 16).unwrap_or_else(|| LoadedIcon {
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
    notify_data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    notify_data.uCallbackMessage = WM_TRAYICON;
    notify_data.hIcon = icon.handle;
    let tip = if mode == TrayMode::Tui {
        "Sheppard dashboard (double-click to open, right-click for menu)"
    } else {
        "Sheppard daemon (right-click for menu)"
    };
    fill_wide_field(&mut notify_data.szTip, tip);

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
unsafe fn watch_tui_minimize(state: std::sync::Arc<TuiTrayState>) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{IsIconic, IsWindowVisible, ShowWindow, SW_HIDE};

    let console_hwnd = state.console_hwnd as HWND;
    if console_hwnd.is_null() {
        return;
    }

    while !state.exit_requested.load(Ordering::SeqCst) {
        // When user clicks the taskbar minimize, the window becomes iconic.
        // We hide it so it disappears from the taskbar entirely; the tray
        // icon remains as the only entry point.
        if IsWindowVisible(console_hwnd) != 0 && IsIconic(console_hwnd) != 0 {
            ShowWindow(console_hwnd, SW_HIDE);
            state.hidden.store(true, Ordering::SeqCst);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(windows)]
unsafe fn open_from_tray() {
    match TRAY_MODE.load(std::sync::atomic::Ordering::SeqCst) {
        value if value == TrayMode::Tui as u32 => restore_tui_window(),
        value if value == TrayMode::Daemon as u32 => open_dashboard_process(),
        _ => {}
    }
}

#[cfg(windows)]
unsafe fn show_tray_menu(hwnd: winapi::shared::windef::HWND, x: i32, y: i32) {
    use std::mem::zeroed;
    use std::ptr::null_mut;
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
        TrackPopupMenu, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_NULL,
    };

    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }

    let open_label = wide_null("Open dashboard");
    let exit_label = wide_null("Exit Sheppard");
    AppendMenuW(menu, MF_STRING, TRAY_CMD_OPEN as usize, open_label.as_ptr());
    AppendMenuW(menu, MF_STRING, TRAY_CMD_EXIT as usize, exit_label.as_ptr());

    // Fall back to the cursor position if the notify-icon anchor was zeroed.
    let (anchor_x, anchor_y) = if x == 0 && y == 0 {
        let mut point: POINT = zeroed();
        if GetCursorPos(&mut point) != 0 {
            (point.x, point.y)
        } else {
            (0, 0)
        }
    } else {
        (x, y)
    };

    // Required so the popup menu dismisses correctly on outside-click.
    SetForegroundWindow(hwnd);
    TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
        anchor_x,
        anchor_y,
        0,
        hwnd,
        null_mut(),
    );
    // Per MSDN: post a benign message so the menu dismisses on outside-click.
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
    // Selected items are dispatched as WM_COMMAND in window_proc.
}

#[cfg(windows)]
unsafe fn restore_tui_window() {
    use std::sync::atomic::Ordering;
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};

    let Some(state) = TUI_TRAY_STATE.get() else {
        return;
    };
    let console_hwnd = state.console_hwnd as HWND;
    if console_hwnd.is_null() {
        return;
    }

    ShowWindow(console_hwnd, SW_SHOW);
    ShowWindow(console_hwnd, SW_RESTORE);
    SetForegroundWindow(console_hwnd);
    state.hidden.store(false, Ordering::SeqCst);
}

#[cfg(windows)]
unsafe fn request_exit_from_tray() {
    use std::sync::atomic::Ordering;
    use winapi::um::winuser::PostQuitMessage;

    match TRAY_MODE.load(Ordering::SeqCst) {
        value if value == TrayMode::Tui as u32 => {
            if let Some(state) = TUI_TRAY_STATE.get() {
                state.exit_requested.store(true, Ordering::SeqCst);
            }
            PostQuitMessage(0);
        }
        value if value == TrayMode::Daemon as u32 => std::process::exit(0),
        _ => {}
    }
}

#[cfg(windows)]
unsafe fn open_dashboard_process() {
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };

    let mut command = std::process::Command::new(current_exe);
    command.arg("tui");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NEW_CONSOLE);
    }

    let _ = command.spawn();
}

#[cfg(windows)]
struct LoadedIcon {
    handle: winapi::shared::windef::HICON,
    owned: bool,
}

#[cfg(windows)]
unsafe fn load_icon_at_size(width: i32, height: i32) -> Option<LoadedIcon> {
    use std::ptr::null_mut;
    use winapi::um::winuser::{LoadImageW, IMAGE_ICON, LR_LOADFROMFILE};

    let icon_path = find_icon_path()?;
    let icon_path = wide_null(&icon_path.to_string_lossy());
    let icon = LoadImageW(
        null_mut(),
        icon_path.as_ptr(),
        IMAGE_ICON,
        width,
        height,
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
