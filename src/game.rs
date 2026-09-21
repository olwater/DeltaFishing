//! 游戏窗口前台检测与单实例互斥。

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, BOOL, HWND, LPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
};

const ERROR_ALREADY_EXISTS: u32 = 183;

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 返回当前前台窗口所属进程的可执行文件名（无路径）。
#[cfg(windows)]
pub fn foreground_exe() -> Option<String> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 512];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        std::path::Path::new(&full)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
    }
}

#[cfg(windows)]
pub fn is_game_in_foreground(game_exe: &str) -> bool {
    let Some(exe) = foreground_exe() else { return false };
    // 前缀匹配：实际游戏进程名可能与配置略有出入（版本/平台后缀等），
    // 只比较前 12 个字符（如 "DeltaForceCl"），不区分大小写。
    let needle: Vec<char> = game_exe
        .chars()
        .take(12)
        .flat_map(char::to_lowercase)
        .collect();
    if needle.is_empty() {
        return false;
    }
    let hay: Vec<char> = exe
        .chars()
        .take(needle.len())
        .flat_map(char::to_lowercase)
        .collect();
    needle == hay
}

/// 按进程名前缀查找游戏顶层窗口（后台模式点击的目标）。
/// 匹配规则与 `is_game_in_foreground` 一致（前 12 字符、不区分大小写）。
#[cfg(windows)]
#[allow(dead_code)]
pub fn find_game_window(game_exe: &str) -> Option<isize> {
    let needle: Vec<char> = game_exe
        .chars()
        .take(12)
        .flat_map(char::to_lowercase)
        .collect();
    if needle.is_empty() {
        return None;
    }
    struct Finder<'a> {
        needle: &'a [char],
        found: isize,
    }
    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let f = unsafe { &mut *(lparam as *mut Finder) };
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return 1;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return 1;
        }
        let mut buf = [0u16; 512];
        let mut size = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return 1;
        }
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        let name = std::path::Path::new(&full)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let hay: Vec<char> = name
            .chars()
            .take(f.needle.len())
            .flat_map(char::to_lowercase)
            .collect();
        if hay == f.needle {
            f.found = hwnd as isize;
            0
        } else {
            1
        }
    }

    let mut f = Finder {
        needle: &needle,
        found: 0,
    };
    unsafe {
        EnumWindows(
            Some(enum_cb),
            &mut f as *mut Finder as LPARAM,
        );
    }
    (f.found != 0).then_some(f.found)
}

/// 用命名互斥量保证单实例。返回 false 表示已有实例在运行。
#[cfg(windows)]
pub fn acquire_single_instance() -> bool {
    let name = to_wide("Global\\DeltaFishingSingleInstance");
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            return false;
        }
        // 成功创建并拥有互斥量 => GetLastError 不是 ERROR_ALREADY_EXISTS
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

// 非 Windows 占位。
#[cfg(not(windows))]
pub fn foreground_exe() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn is_game_in_foreground(_game_exe: &str) -> bool {
    false
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn find_game_window(_game_exe: &str) -> Option<isize> {
    None
}

#[cfg(not(windows))]
pub fn acquire_single_instance() -> bool {
    true
}