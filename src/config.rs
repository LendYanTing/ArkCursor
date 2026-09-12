//! Persistence: config.toml next to the executable.
//! Each cursor file carries its own profile (size / update rate) - switching
//! the file switches the whole configuration (spec: 一个图标就是一个配置).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    /// rendered cursor width in px
    #[serde(default = "default_size")]
    pub size: u32,
    /// 0 = follow the monitor refresh rate
    #[serde(default)]
    pub update_rate: u32,
}

fn default_size() -> u32 {
    48
}

impl Profile {
    pub fn effective_rate(&self, screen_rate: u32) -> u32 {
        if self.update_rate >= 30 {
            self.update_rate
        } else {
            screen_rate.max(60)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// process image name to match (case-insensitive)
    #[serde(default = "default_process")]
    pub process: String,
    /// window title substring to match (either one matches)
    #[serde(default = "default_title")]
    pub window_title: String,
    /// active cursor file
    #[serde(default)]
    pub cursor_path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// per-cursor-file profiles, keyed by normalized path
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

fn default_process() -> String {
    "arknights.exe".into()
}
fn default_title() -> String {
    "明日方舟".into()
}
fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            process: default_process(),
            window_title: default_title(),
            cursor_path: String::new(),
            enabled: true,
            profiles: BTreeMap::new(),
        }
    }
}

fn normalize_path(s: &str) -> String {
    s.to_lowercase().replace('/', "\\")
}

pub fn config_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    p.set_file_name("config.toml");
    p
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(s) => {
                    log::info!("config loaded: {}", path.display());
                    s
                }
                Err(e) => {
                    log::warn!("config parse failed ({e}), using defaults");
                    Self::default()
                }
            },
            Err(_) => {
                log::info!("no config at {}, using defaults", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let path = config_path();
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }

    /// Profile for the active cursor file (created with defaults if absent).
    pub fn active_profile(&self) -> Profile {
        self.profiles
            .get(&normalize_path(&self.cursor_path))
            .cloned()
            .unwrap_or(Profile {
                size: 48,
                update_rate: 0,
            })
    }

    pub fn set_active_profile(&mut self, p: Profile) {
        if self.cursor_path.is_empty() {
            return;
        }
        self.profiles
            .insert(normalize_path(&self.cursor_path), p);
    }
}
