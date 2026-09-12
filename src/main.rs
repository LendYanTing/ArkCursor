//! ArkCursor - in-game custom cursor via AttachThreadInput + ShowCursor,
//! with an egui GUI, auto-elevation, and persistent settings.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod cursor;
mod elevate;
mod logger;
mod overlay;
mod process_list;
mod runtime;

use config::Settings;

fn main() -> eframe::Result<()> {
    // logs go to latest.log next to the exe (no console in release builds)
    let log_path = {
        let mut p = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("."));
        p.set_file_name("latest.log");
        p
    };
    let _ = logger::init(&log_path);
    log::info!("ArkCursor starting (elevated check next)");

    // Per-Monitor DPI V2 before anything renders.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // Auto-elevation (AttachThreadInput across integrity levels is denied).
    if !elevate::is_elevated() {
        log::info!("not elevated; relaunching via runas");
        if elevate::relaunch_elevated() {
            return Ok(()); // elevated instance takes over
        }
        log::warn!("elevation declined; Attach will fail for elevated games");
    }

    // Detect the primary monitor refresh rate (default update rate source).
    let screen_rate = detect_screen_refresh().unwrap_or(60);
    log::info!("screen refresh rate: {} Hz", screen_rate);

    // Settings + default cursor extraction (persisted next to the exe).
    let mut settings = Settings::load();
    if settings.cursor_path.is_empty() {
        let default_path = config::config_path()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("default.cur");
        if !default_path.exists() {
            let _ = std::fs::write(&default_path, include_bytes!("../assets/default.cur"));
        }
        settings.cursor_path = default_path.to_string_lossy().to_string();
        settings.save();
    }

    let shared = runtime::Shared::new(settings.clone());
    shared.screen_rate.store(screen_rate, std::sync::atomic::Ordering::SeqCst);
    {
        let shared = shared.clone();
        std::thread::Builder::new()
            .name("runtime".into())
            .spawn(move || runtime::run(shared))
            .expect("spawn runtime");
    }

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 640.0])
            .with_title("ArkCursor")
            .with_taskbar(true),
        ..Default::default()
    };
    eframe::run_native(
        "ArkCursor",
        native,
        Box::new(move |cc| {
            install_cjk_fonts(cc);
            force_light_theme(cc);
            let prof = settings.active_profile();
            Ok(Box::new(app::ArkCursorApp {
                shared,
                settings,
                screen_rate,
                processes: Vec::new(),
                process_list_open: false,
                texture: None,
                texture_key: 0,
                edit_size: prof.size,
                edit_rate: prof.update_rate,
                follow_screen: prof.update_rate < 30,
                theme_forced: false,
            }))
        }),
    )
}

/// Primary monitor refresh rate via EnumDisplaySettingsW.
fn detect_screen_refresh() -> Option<u32> {
    use windows::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    };
    unsafe {
        let mut dm = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if EnumDisplaySettingsW(
            windows::core::PCWSTR::null(),
            ENUM_CURRENT_SETTINGS,
            &mut dm,
        )
        .as_bool()
        {
            let freq = dm.dmDisplayFrequency;
            if freq > 0 {
                return Some(freq);
            }
        }
        None
    }
}

/// Query a process image base name with minimal rights.
pub fn process_image_name(pid: u32) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if ok.is_ok() && len > 0 {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            full.rsplit(|c| c == char::from_u32(92).unwrap() || c == '/').next().map(|s| s.to_string())
        } else {
            None
        }
    }
}

/// DWM extended frame bounds (physical px, excludes invisible borders).
pub fn extended_frame_bounds(
    hwnd: windows::Win32::Foundation::HWND,
) -> Option<windows::Win32::Foundation::RECT> {
    use windows::Win32::Foundation::RECT;
    unsafe {
        let mut rect = RECT::default();
        windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()
        .map(|_| rect)
    }
}

/// Load a Chinese-capable font so egui doesn't render tofu squares.
fn install_cjk_fonts(cc: &eframe::CreationContext<'_>) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",     // 微软雅黑
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\simhei.ttf",   // 黑体
        r"C:\Windows\Fonts\simsun.ttc",   // 宋体
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.insert(0, "cjk".into());
            }
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.push("cjk".into());
            }
            cc.egui_ctx.set_fonts(fonts);
            log::info!("CJK font loaded: {path}");
            return;
        }
    }
    log::warn!("no CJK font found among {CANDIDATES:?}; Chinese text may be boxes");
}

/// Force a consistent light theme (mixed dark/light panels hurt readability).
fn force_light_theme(cc: &eframe::CreationContext<'_>) {
    cc.egui_ctx.set_theme(egui::ThemePreference::Light);
}
