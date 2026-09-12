//! Running-process enumeration for the picker (visible top-level windows).
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowLongPtrW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE,
};

#[derive(Debug, Clone)]
pub struct ProcEntry {
    pub pid: u32,
    pub exe: String,
    pub title: String,
    pub class: String,
}

pub fn list_window_processes() -> Vec<ProcEntry> {
    let mut out: Vec<ProcEntry> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut out as *mut _ as isize));
    }
    out.sort_by(|a, b| a.exe.cmp(&b.exe));
    out.dedup_by(|a, b| a.exe == b.exe && a.title == b.title);
    out
}

unsafe extern "system" fn enum_cb(
    hwnd: windows::Win32::Foundation::HWND,
    lp: LPARAM,
) -> windows::core::BOOL {
    let out = &mut *(lp.0 as *mut Vec<ProcEntry>);
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return windows::core::BOOL(1);
        }
        let ex = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex & 0x80 != 0 {
            return windows::core::BOOL(1);
        }
        let mut pid = 0u32;
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return windows::core::BOOL(1);
        }
        let mut title_buf = [0u16; 512];
        let tlen = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut title_buf);
        let title = String::from_utf16_lossy(&title_buf[..tlen.max(0) as usize]);
        if title.is_empty() {
            return windows::core::BOOL(1);
        }
        let mut class_buf = [0u16; 256];
        windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut class_buf);
        let class = String::from_utf16_lossy(&class_buf[..]);

        let exe = crate::process_image_name(pid).unwrap_or_default();
        if exe.is_empty() {
            return windows::core::BOOL(1);
        }
        out.push(ProcEntry {
            pid,
            exe,
            title,
            class,
        });
    }
    windows::core::BOOL(1)
}

// RECT import used by callers? keep unused minimal
#[allow(dead_code)]
fn _unused(r: RECT) {}
