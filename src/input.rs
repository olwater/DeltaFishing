//! 鼠标输入模拟（SendInput）与全局热键轮询（F8 暂停 / F9 退出）。

use std::thread;
use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
};

#[cfg(windows)]
fn send_input(flag: u32) {
    let input = INPUT {
        r#type: 0, // INPUT_MOUSE
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 点击一次鼠标左键（按下 - 保持 hold - 松开）。
#[cfg(windows)]
pub fn left_click(hold: f64) {
    send_input(MOUSEEVENTF_LEFTDOWN);
    thread::sleep(Duration::from_secs_f64(hold));
    send_input(MOUSEEVENTF_LEFTUP);
    thread::sleep(Duration::from_secs_f64(hold));
}

/// 后台左键点击：向目标窗口投递鼠标消息（不动真实鼠标、不需前台）。
///
/// 使用客户区中心，避免真实鼠标移到其他应用时产生窗口外坐标。
/// 投递成功仅代表进入消息队列，不代表游戏已执行动作。
#[cfg(windows)]
pub fn post_left_click(hwnd: isize, hold: f64) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{GetLastError, RECT};
    use windows_sys::Win32::System::SystemServices::MK_LBUTTON;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, IsWindow, IsIconic, PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP,
    };
    unsafe {
        if IsWindow(hwnd as _) == 0 || IsIconic(hwnd as _) != 0 {
            return Err("游戏窗口不存在或已最小化".into());
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetClientRect(hwnd as _, &mut rect) == 0 {
            return Err(format!("读取游戏客户区失败，Windows 错误 {}", GetLastError()));
        }
        let (x, y) = ((rect.right - rect.left) / 2, (rect.bottom - rect.top) / 2);
        if x <= 0 || y <= 0 { return Err("游戏客户区尺寸无效".into()); }
        let lparam = (((y as u32 & 0xffff) << 16) | (x as u32 & 0xffff)) as isize;
        if PostMessageW(hwnd as _, WM_LBUTTONDOWN, MK_LBUTTON as usize, lparam) == 0 {
            return Err(format!("后台按下消息失败，Windows 错误 {}", GetLastError()));
        }
        thread::sleep(Duration::from_secs_f64(hold));
        if PostMessageW(hwnd as _, WM_LBUTTONUP, 0, lparam) == 0 {
            return Err(format!("后台松开消息失败，Windows 错误 {}", GetLastError()));
        }
    }
    thread::sleep(Duration::from_secs_f64(hold));
    Ok(())
}

/// 右键状态句柄：记住当前是否已按住，避免重复按下；用于等待咬钩时屏蔽他人声音。
#[cfg(windows)]
#[derive(Default)]
pub struct RightHold {
    down: bool,
}

#[cfg(windows)]
impl RightHold {
    pub fn ensure_down(&mut self) {
        if !self.down {
            send_input(MOUSEEVENTF_RIGHTDOWN);
            self.down = true;
        }
    }
    pub fn ensure_up(&mut self) {
        if self.down {
            send_input(MOUSEEVENTF_RIGHTUP);
            self.down = false;
        }
    }
}

// 非 Windows 平台占位（无输入模拟）。
#[cfg(not(windows))]
pub fn left_click(_hold: f64) {}

#[cfg(not(windows))]
#[derive(Default)]
pub struct RightHold {}

#[cfg(not(windows))]
impl RightHold {
    pub fn ensure_down(&mut self) {}
    pub fn ensure_up(&mut self) {}
}

const VK_F8: i32 = 0x77;
const VK_F9: i32 = 0x78;

#[cfg(windows)]
fn key_down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

#[cfg(not(windows))]
fn key_down(_vk: i32) -> bool {
    false
}

pub enum Hotkey {
    None,
    Pause,
    Quit,
}

/// 边缘触发的热键轮询器：F8 暂停/继续，F9 退出。
#[derive(Default)]
pub struct Hotkeys {
    prev_f8: bool,
    prev_f9: bool,
}

impl Hotkeys {
    pub fn poll(&mut self) -> Hotkey {
        let f8 = key_down(VK_F8);
        let f9 = key_down(VK_F9);
        let result = if f9 && !self.prev_f9 {
            Hotkey::Quit
        } else if f8 && !self.prev_f8 {
            Hotkey::Pause
        } else {
            Hotkey::None
        };
        self.prev_f8 = f8;
        self.prev_f9 = f9;
        result
    }
}