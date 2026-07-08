//! 壁纸窗口平台适配：macOS 用桌面窗口层级（objc2），Windows 用 WorkerW 注入。

/// 把窗口钉到"壁纸层"（桌面图标之下、系统壁纸之上）。
pub fn attach(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    macos::set_desktop_level(window);
    #[cfg(target_os = "windows")]
    windows_impl::attach(window);
    let _ = window;
}

/// 互动模式切换：macOS 在桌面层/普通层间切换以接收点击；Windows 暂为 no-op
/// （WorkerW 注入后窗口在桌面图标之下，交互能力有限，留待后续迭代）。
pub fn set_interactive(window: &tauri::WebviewWindow, on: bool) {
    #[cfg(target_os = "macos")]
    macos::set_window_level(window, if on { 4 } else { 2 });
    let _ = (window, on);
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowLevelForKey(key: i32) -> i32;
    }

    /// key: 2 = kCGDesktopWindowLevelKey（桌面层），4 = kCGNormalWindowLevelKey（普通层）。
    pub fn set_window_level(window: &tauri::WebviewWindow, key: i32) {
        let Ok(ptr) = window.ns_window() else { return };
        let ns_window = ptr as *mut AnyObject;
        unsafe {
            let level = CGWindowLevelForKey(key);
            let _: () = msg_send![ns_window, setLevel: level as isize];
        }
    }

    pub fn set_desktop_level(window: &tauri::WebviewWindow) {
        set_window_level(window, 2);
        let Ok(ptr) = window.ns_window() else { return };
        let ns_window = ptr as *mut AnyObject;
        unsafe {
            // canJoinAllSpaces (1<<0) | stationary (1<<4) | ignoresCycle (1<<6)
            let behavior: usize = (1 << 0) | (1 << 4) | (1 << 6);
            let _: () = msg_send![ns_window, setCollectionBehavior: behavior];
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, SendMessageTimeoutW, SetParent, SMTO_NORMAL,
    };

    fn null_hwnd() -> HWND {
        HWND(std::ptr::null_mut())
    }

    /// WorkerW 注入：向 Progman 发 0x052C 催生 WorkerW 窗口，
    /// 找到与 SHELLDLL_DefView 同级的那个 WorkerW，把壁纸窗口 SetParent 进去，
    /// 使其位于桌面图标之下、系统壁纸之上。找不到时回退挂到 Progman。
    pub fn attach(window: &tauri::WebviewWindow) {
        let Ok(h) = window.hwnd() else { return };
        let hwnd = HWND(h.0 as *mut _);
        unsafe {
            let progman = FindWindowW(w!("Progman"), PCWSTR::null()).unwrap_or_else(|_| null_hwnd());
            if !progman.is_invalid() {
                let mut res: usize = 0;
                let _ = SendMessageTimeoutW(
                    progman,
                    0x052C,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_NORMAL,
                    1000,
                    Some(&mut res as *mut usize),
                );
            }
            let mut worker: Option<HWND> = None;
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut worker as *mut _ as isize));
            let target = worker.unwrap_or(progman);
            if !target.is_invalid() {
                let _ = SetParent(hwnd, target);
            }
        }
    }

    unsafe extern "system" fn enum_proc(top: HWND, lparam: LPARAM) -> BOOL {
        let shell =
            FindWindowExW(top, null_hwnd(), w!("SHELLDLL_DefView"), PCWSTR::null()).unwrap_or_else(|_| null_hwnd());
        if !shell.is_invalid() {
            if let Ok(worker) = FindWindowExW(null_hwnd(), top, w!("WorkerW"), PCWSTR::null()) {
                if !worker.is_invalid() {
                    *(lparam.0 as *mut Option<HWND>) = Some(worker);
                    return BOOL(0); // 找到即停止枚举
                }
            }
        }
        BOOL(1) // 继续
    }
}
