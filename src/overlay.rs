//! Layered overlay window (port from CursorOverlay), plus in-place sprite
//! replacement via UpdateLayeredWindow.
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS, HDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, RegisterClassExW, ShowWindow, UpdateLayeredWindow, SW_HIDE, ULW_ALPHA,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::w;

const CLASS_NAME: windows::core::PCWSTR = w!("ArkCursorOverlay");

pub struct Overlay {
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
    hdc_mem: HDC,
    hbm: windows::Win32::Graphics::Gdi::HBITMAP,
    visible: bool,
}

extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp) }
}

fn apply_sprite(
    hwnd: HWND,
    sprite: &crate::cursor::CursorImage,
    old_dc: &mut Option<HDC>,
    old_bm: &mut Option<windows::Win32::Graphics::Gdi::HBITMAP>,
) -> Result<(), String> {
    unsafe {
        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: sprite.width as i32,
                biHeight: -(sprite.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm = CreateDIBSection(Some(hdc_mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("CreateDIBSection: {e}"))?;
        if bits.is_null() {
            return Err("CreateDIBSection null bits".into());
        }
        // straight RGBA -> premultiplied BGRA (ULW requirement)
        let mut premul = vec![0u8; sprite.rgba.len()];
        for i in 0..sprite.rgba.len() / 4 {
            let a = sprite.rgba[i * 4 + 3] as u16;
            premul[i * 4] = (sprite.rgba[i * 4 + 2] as u16 * a / 255) as u8;
            premul[i * 4 + 1] = (sprite.rgba[i * 4 + 1] as u16 * a / 255) as u8;
            premul[i * 4 + 2] = (sprite.rgba[i * 4] as u16 * a / 255) as u8;
            premul[i * 4 + 3] = sprite.rgba[i * 4 + 3];
        }
        std::ptr::copy_nonoverlapping(premul.as_ptr(), bits as *mut u8, premul.len());
        SelectObject(hdc_mem, hbm.into());
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE {
            cx: sprite.width as i32,
            cy: sprite.height as i32,
        };
        UpdateLayeredWindow(
            hwnd,
            Some(hdc_screen),
            Some(&POINT { x: 0, y: 0 }),
            Some(&size),
            Some(hdc_mem),
            Some(&POINT { x: 0, y: 0 }),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
        .map_err(|e| format!("UpdateLayeredWindow: {e}"))?;
        ReleaseDC(None, hdc_screen);

        // free previous resources
        if let Some(prev_dc) = old_dc.take() {
            let _ = DeleteDC(prev_dc);
        }
        if let Some(prev_bm) = old_bm.take() {
            let _ = DeleteObject(prev_bm.into());
        }
        *old_dc = Some(hdc_mem);
        *old_bm = Some(hbm);
    }
    Ok(())
}

impl Overlay {
    pub fn create(sprite: &crate::cursor::CursorImage) -> Result<Self, String> {
        unsafe {
            let hmodule = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let hinstance = windows::Win32::Foundation::HINSTANCE(hmodule.0);
            static REGISTER: std::sync::Once = std::sync::Once::new();
            REGISTER.call_once(|| {
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: hinstance,
                    lpszClassName: CLASS_NAME,
                    ..Default::default()
                };
                RegisterClassExW(&wc);
            });
            let ex = WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_NOACTIVATE
                | WS_EX_TOOLWINDOW
                | WS_EX_TOPMOST;
            let hwnd = CreateWindowExW(
                ex,
                CLASS_NAME,
                w!("ArkCursor"),
                WS_POPUP,
                0,
                0,
                sprite.width as i32,
                sprite.height as i32,
                None,
                None,
                Some(hinstance),
                None,
            )
            .map_err(|e| format!("CreateWindowExW: {e}"))?;

            let mut old_dc = None;
            let mut old_bm = None;
            apply_sprite(hwnd, sprite, &mut old_dc, &mut old_bm)?;
            ShowWindow(hwnd, SW_HIDE);

            Ok(Overlay {
                hwnd,
                width: sprite.width,
                height: sprite.height,
                hdc_mem: old_dc.unwrap(),
                hbm: old_bm.unwrap(),
                visible: false,
            })
        }
    }

    /// Replace the bitmap in place (cursor file / size changed).
    pub fn update_sprite(&mut self, sprite: &crate::cursor::CursorImage) -> Result<(), String> {
        let mut old_dc = Some(self.hdc_mem);
        let mut old_bm = Some(self.hbm);
        let r = apply_sprite(self.hwnd, sprite, &mut old_dc, &mut old_bm);
        if let (Some(dc), Some(bm)) = (old_dc, old_bm) {
            self.hdc_mem = dc;
            self.hbm = bm;
        }
        self.width = sprite.width;
        self.height = sprite.height;
        r
    }

    pub fn place_at(&mut self, x: i32, y: i32, hot_x: i32, hot_y: i32) -> bool {
        unsafe {
            let r = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                self.hwnd,
                Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
                x - hot_x,
                y - hot_y,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
            );
            self.visible = true;
            r.is_ok()
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.hbm.into());
            let _ = DeleteDC(self.hdc_mem);
        }
    }
}
