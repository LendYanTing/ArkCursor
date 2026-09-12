//! File logger: everything goes to latest.log next to the executable
//! (truncated per run) - there is no console window in release builds.
use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

pub struct FileLogger {
    file: Mutex<File>,
}

impl log::Log for FileLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= log::LevelFilter::Info
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| {
                let s = d.as_secs() % 86400;
                format!("{:02}:{:02}:{:02}.{:03}", s / 3600, s / 60 % 60, s % 60,
                        d.subsec_millis())
            })
            .unwrap_or_default();
        let line = format!(
            "{} [{:<5}] {}: {}\n",
            ts,
            record.level(),
            record.module_path().unwrap_or(""),
            record.args()
        );
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
    fn flush(&self) {}
}

/// Init; returns false when the file could not be opened.
pub fn init(path: &std::path::Path) -> bool {
    match File::create(path) {
        Ok(f) => {
            let _ = log::set_boxed_logger(Box::new(FileLogger {
                file: Mutex::new(f),
            }));
            log::set_max_level(log::LevelFilter::Info);
            true
        }
        Err(_) => false,
    }
}
