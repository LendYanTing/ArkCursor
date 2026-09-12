//! Worker thread: finds the target window, attaches on foreground, hides the
//! original cursor, draws the overlay cursor. Owns the Win32 overlay window.
use crate::config::Settings;
use crate::cursor::CursorImage;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{GetLastError, POINT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetForegroundWindow, ShowCursor, CURSORINFO, CURSOR_SHOWING,
};

pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub found: bool,
    pub attached: bool,
    pub pid: u32,
    pub hwnd: isize,
    pub exe: String,
    pub cursor_hidden: bool,
    pub error: String,
    pub fps: f32,
}

#[derive(Clone)]
pub struct Shared {
    pub settings: Arc<Mutex<Settings>>,
    pub sprite: Arc<Mutex<Option<Arc<CursorImage>>>>,
    pub status: Arc<Mutex<Status>>,
    pub settings_version: Arc<AtomicU64>,
    /// detected monitor refresh rate (set at startup)
    pub screen_rate: Arc<AtomicU32>,
}

impl Shared {
    pub fn new(settings: Settings) -> Self {
        Shared {
            settings: Arc::new(Mutex::new(settings)),
            sprite: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(Status::default())),
            settings_version: Arc::new(AtomicU64::new(0)),
            screen_rate: Arc::new(AtomicU32::new(60)),
        }
    }

    pub fn bump_settings(&self) {
        self.settings_version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn set_status(&self, f: impl FnOnce(&mut Status)) {
        let mut st = self.status.lock().unwrap();
        f(&mut st);
    }
}

fn read_ci() -> Option<CURSORINFO> {
    unsafe {
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        GetCursorInfo(&mut ci).ok().map(|_| ci)
    }
}

pub fn run(shared: Shared) {
    let mut period = Duration::from_millis(1000 / 120);
    let overlay_tid = unsafe { GetCurrentThreadId() };

    let mut loaded_version = u64::MAX;
    let mut overlay: Option<crate::overlay::Overlay> = None;
    let mut attached = false;
    let mut attached_tid = 0u32;
    let mut target: Option<TargetInfo> = None;
    let mut last_find = Instant::now() - Duration::from_secs(1);
    let mut last_mouse = (-1i32, -1i32);
    let mut frames = 0u64;
    let mut fps_window = Instant::now();

    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break;
        }
        frames += 1;
        if fps_window.elapsed() >= Duration::from_secs(1) {
            let fps = frames as f32 / fps_window.elapsed().as_secs_f32();
            shared.set_status(|s| s.fps = fps);
            frames = 0;
            fps_window = Instant::now();
        }

        let settings = shared.settings.lock().unwrap().clone();
        let enabled = settings.enabled;
        let prof = settings.active_profile();
        let rate = prof.effective_rate(shared.screen_rate.load(Ordering::SeqCst)).clamp(30, 240);
        period = Duration::from_millis(1000 / rate as u64);

        // ---- sprite reload on settings change --------------------------------
        if loaded_version != shared.settings_version.load(Ordering::SeqCst) {
            loaded_version = shared.settings_version.load(Ordering::SeqCst);
            let path = settings.cursor_path.clone();
            match crate::cursor::load(std::path::Path::new(&path), prof.size) {
                Ok(img) => {
                    let img = Arc::new(img);
                    match overlay.as_mut() {
                        Some(o) => {
                            if let Err(e) = o.update_sprite(&img) {
                                shared.set_status(|s| {
                                    s.error = format!("更新光标失败: {e}")
                                });
                            }
                        }
                        None => {
                            // create the overlay window on first successful load
                            match crate::overlay::Overlay::create(&img) {
                                Ok(o) => {
                                    overlay = Some(o);
                                    last_mouse = (-1, -1); // force placement
                                    log::info!(
                                        "overlay window created: {}x{}",
                                        img.width,
                                        img.height
                                    );
                                }
                                Err(e) => shared.set_status(|s| {
                                    s.error = format!("overlay 创建失败: {e}")
                                }),
                            }
                        }
                    }
                    *shared.sprite.lock().unwrap() = Some(img);
                    shared.set_status(|s| s.error = String::new());
                }
                Err(e) => {
                    shared.set_status(|s| s.error = format!("光标加载失败: {e}"));
                }
            }
        }

        // ---- target discovery -------------------------------------------------
        let find_now = match &target {
            Some(t) => !unsafe {
                windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(
                    windows::Win32::Foundation::HWND(t.hwnd as *mut core::ffi::c_void),
                ))
                .as_bool()
            },
            None => last_find.elapsed() >= Duration::from_millis(500),
        };
        if find_now {
            target = find_target(&settings);
            last_find = Instant::now();
            if let Some(t) = &target {
                shared.set_status(|s| {
                    s.found = true;
                    s.pid = t.pid;
                    s.hwnd = t.hwnd;
                    s.exe = t.exe.clone();
                });
            } else {
                shared.set_status(|s| {
                    s.found = false;
                    s.attached = false;
                });
            }
        }

        // ---- foreground gating: attach + hide / detach + restore --------------
        let fg = target
            .as_ref()
            .map(|t| unsafe {
                !GetForegroundWindow().is_invalid()
                    && GetForegroundWindow()
                        == windows::Win32::Foundation::HWND(t.hwnd as *mut _)
            })
            .unwrap_or(false)
            && enabled;

        if fg && !attached {
            if let Some(t) = &target {
                let r = unsafe { AttachThreadInput(overlay_tid, t.tid, true) };
                if r.as_bool() {
                    attached = true;
                    attached_tid = t.tid;
                    unsafe {
                        let mut n = ShowCursor(false);
                        while n >= 0 {
                            n = ShowCursor(false);
                        }
                    }
                    shared.set_status(|s| {
                        s.attached = true;
                        s.error = String::new();
                    });
                    log::info!("attached; original cursor hidden");
                } else {
                    let gle = unsafe { GetLastError().0 };
                    shared.set_status(|s| {
                        s.error = if gle == 5 {
                            "Attach 被拒绝：请以管理员身份运行本工具".into()
                        } else {
                            format!("Attach 失败 GLE={gle}")
                        }
                    });
                    std::thread::sleep(Duration::from_millis(1000));
                }
            }
        } else if (!fg || !enabled) && attached {
            detach_and_restore(overlay_tid, attached_tid);
            attached = false;
            shared.set_status(|s| s.attached = false);
        }

        // game exited while attached -> detach immediately
        if attached && target.is_none() {
            detach_and_restore(overlay_tid, attached_tid);
            attached = false;
            shared.set_status(|s| s.attached = false);
        }

        // ---- overlay placement ------------------------------------------------
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        if attached {
            let (hx, hy) = shared
                .sprite
                .lock()
                .unwrap()
                .as_ref()
                .map(|s| s.hotspot)
                .unwrap_or((0, 0));
            if (pt.x, pt.y) != last_mouse {
                if let Some(o) = overlay.as_mut() {
                    o.place_at(pt.x, pt.y, hx, hy);
                }
            }
        } else if let Some(o) = overlay.as_mut() {
            o.hide();
        }
        last_mouse = (pt.x, pt.y);

        // hidden-state reporting (informational)
        if attached {
            if let Some(ci) = read_ci() {
                let showing = (ci.flags.0 & CURSOR_SHOWING.0) != 0;
                shared.set_status(|s| s.cursor_hidden = !showing);
            }
        }

        std::thread::sleep(period);
    }

    // ---- shutdown: restore everything -----------------------------------------
    if attached {
        detach_and_restore(overlay_tid, attached_tid);
    }
    if let Some(o) = overlay.as_mut() {
        o.hide();
    }
    log::info!("runtime stopped");
}

fn detach_and_restore(overlay_tid: u32, target_tid: u32) {
    // restore the SHARED display count BEFORE splitting queues (leak fix)
    unsafe {
        let mut n = ShowCursor(true);
        while n < 0 {
            n = ShowCursor(true);
        }
        let r = AttachThreadInput(overlay_tid, target_tid, false);
        log::info!("detached ({}) and display count restored", r.as_bool());
        // make sure OUR queue count is exactly 0
        let mut n = ShowCursor(false);
        while n > 0 {
            n = ShowCursor(false);
        }
        let mut n = ShowCursor(true);
        while n < 0 {
            n = ShowCursor(true);
        }
        let _ = n;
    }
}

struct TargetInfo {
    hwnd: isize,
    pid: u32,
    tid: u32,
    exe: String,
}

fn find_target(settings: &Settings) -> Option<TargetInfo> {
    struct Ctx {
        proc_filter: String,
        title_filter: String,
        best: Option<TargetInfo>,
        best_area: i64,
    }
    let mut ctx = Ctx {
        proc_filter: settings.process.trim().to_lowercase(),
        title_filter: settings.window_title.trim().to_lowercase(),
        best: None,
        best_area: -1,
    };
    if ctx.proc_filter.is_empty() && ctx.title_filter.is_empty() {
        return None;
    }
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
            Some(enum_proc),
            windows::Win32::Foundation::LPARAM(&mut ctx as *mut Ctx as isize),
        );
    }
    ctx.best
}

unsafe extern "system" fn enum_proc(
    hwnd: windows::Win32::Foundation::HWND,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    let ctx = &mut *(lp.0 as *mut Ctx_);
    unsafe {
        if !windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool() {
            return windows::core::BOOL(1);
        }
        let ex = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE) as u32;
        if ex & 0x80 != 0 {
            return windows::core::BOOL(1); // WS_EX_TOOLWINDOW
        }
        let mut pid = 0u32;
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return windows::core::BOOL(1);
        }
        let mut title_buf = [0u16; 512];
        let tlen = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut title_buf);
        let title = String::from_utf16_lossy(&title_buf[..tlen.max(0) as usize]);

        let exe = crate::process_image_name(pid).unwrap_or_default();
        let match_proc = !ctx.proc_filter.is_empty() && exe.to_lowercase() == ctx.proc_filter;
        let match_title = !ctx.title_filter.is_empty()
            && title.to_lowercase().contains(&ctx.title_filter);
        if !match_proc && !match_title {
            return windows::core::BOOL(1);
        }
        if title.is_empty() && !match_proc {
            return windows::core::BOOL(1); // UIPI may hide elevated titles
        }
        let Some(rect) = crate::extended_frame_bounds(hwnd) else {
            return windows::core::BOOL(1);
        };
        let area = (rect.right - rect.left) as i64 * (rect.bottom - rect.top) as i64;
        if area > ctx.best_area {
            let tid = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, None);
            ctx.best = Some(TargetInfo {
                hwnd: hwnd.0 as isize,
                pid,
                tid,
                exe,
            });
            ctx.best_area = area;
        }
    }
    windows::core::BOOL(1)
}

struct Ctx_ {
    proc_filter: String,
    title_filter: String,
    best: Option<TargetInfo>,
    best_area: i64,
}
