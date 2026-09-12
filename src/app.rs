//! ArkCursor GUI - readability-first layout: status banner on top, settings in
//! two groups, live preview pinned on the right side panel.
use crate::config::{Profile, Settings};
use crate::runtime::Shared;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub struct ArkCursorApp {
    pub shared: Shared,
    pub settings: Settings,
    pub screen_rate: u32,
    pub processes: Vec<crate::process_list::ProcEntry>,
    pub process_list_open: bool,
    pub texture: Option<egui::TextureHandle>,
    pub texture_key: u64,
    /// staged profile edits
    pub edit_size: u32,
    pub edit_rate: u32,
    pub follow_screen: bool,
    pub theme_forced: bool,
}

impl ArkCursorApp {
    fn persist(&mut self) {
        let prof = self.edit_profile();
        self.settings.set_active_profile(prof);
        self.shared
            .settings
            .lock()
            .unwrap()
            .clone_from(&self.settings);
        self.shared.bump_settings();
        self.settings.save();
    }

    fn edit_profile(&mut self) -> Profile {
        Profile {
            size: self.edit_size,
            update_rate: self.edit_rate,
        }
    }

    fn reload_profile_from_active_file(&mut self) {
        let p = self.settings.active_profile();
        self.edit_size = p.size;
        self.edit_rate = p.update_rate;
        self.follow_screen = p.update_rate < 30;
    }
}

impl eframe::App for ArkCursorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // egui_winit applies the OS theme AFTER our creation callback runs, so
        // re-assert light theme on the first frame (keeps panels consistent).
        if !self.theme_forced {
            self.theme_forced = true;
            ctx.set_theme(egui::ThemePreference::Light);
            // title bar follows the winit theme; default is the OS (dark) one
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::viewport::SystemTheme::Light));
        }
        // keep live status (fps / cursor state) updating without user input
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let status = self.shared.status.lock().unwrap().clone();

        // preview texture
        let sprite = self.shared.sprite.lock().unwrap().clone();
        if let Some(sprite) = sprite {
            let key = Arc::as_ptr(&sprite) as *const () as u64 + sprite.width as u64;
            if self.texture_key != key || self.texture.is_none() {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [sprite.width as usize, sprite.height as usize],
                    &sprite.rgba,
                );
                let tex = ctx.load_texture("cursor_preview", img, egui::TextureOptions::NEAREST);
                self.texture = Some(tex);
                self.texture_key = key;
            }
        }

        // ---- right side: preview + live status ----------------------------
        let panel_fill = egui::Color32::from_rgb(248, 248, 250);
        egui::SidePanel::right("preview_panel")
            .resizable(false)
            .exact_width(230.0)
            .frame(egui::Frame::default().fill(panel_fill).inner_margin(12.0))
            .show(ctx, |ui| {
                ui.add_space(10.0);
                ui.strong("实时预览");
                ui.add_space(6.0);
                if let Some(tex) = &self.texture {
                    let size = tex.size_vec2();
                    let k = (190.0 / size.x.max(1.0)).min(2.5);
                    ui.image((tex.id(), size * k));
                } else {
                    ui.label("尚未加载光标");
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);
                ui.strong("运行状态");
                egui::Grid::new("status_grid")
                    .num_columns(2)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        ui.label("刷新率");
                        ui.label(format!("{:.0} fps", status.fps));
                        ui.end_row();
                        ui.label("原光标");
                        ui.label(if status.cursor_hidden {
                            "已隐藏 ✓"
                        } else {
                            "显示中"
                        });
                        ui.end_row();
                        ui.label("窗口 PID");
                        ui.label(if status.found {
                            format!("{}", status.pid)
                        } else {
                            "-".into()
                        });
                        ui.end_row();
                    });
            });

        // ---- main column ---------------------------------------------------
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(panel_fill).inner_margin(14.0))
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.add(egui::Label::new(
                    egui::RichText::new("ArkCursor")
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(45, 45, 50)),
                ));
                ui.add_space(6.0);

                // ---- status banner ----------------------------------------
                let (state_txt, state_color) = if status.attached {
                    ("替换运行中", egui::Color32::from_rgb(80, 200, 90))
                } else if status.found {
                    ("目标已找到（等待游戏前台）", egui::Color32::from_rgb(230, 190, 60))
                } else {
                    ("未找到目标窗口", egui::Color32::from_rgb(230, 80, 80))
                };
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(244, 245, 248))
                    .rounding(6.0)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(state_color, "●");
                            ui.add_space(2.0);
                            ui.add(egui::Label::new(
                                egui::RichText::new(state_txt).size(16.0).strong(),
                            ));
                        });
                        if status.found {
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "进程: {} (PID {})",
                                    status.exe, status.pid
                                ))
                                .weak(),
                            );
                        }
                    });
                if !status.error.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(240, 90, 90), &status.error);
                }

                ui.add_space(10.0);

                // ---- enable ----
                if ui
                    .checkbox(
                        &mut self.settings.enabled,
                        "启用替换（游戏前台时隐藏原光标并显示自定义光标）",
                    )
                    .changed()
                {
                    self.shared
                        .settings
                        .lock()
                        .unwrap()
                        .clone_from(&self.settings);
                    self.settings.save();
                }

                ui.add_space(8.0);

                // ---- target group ----
                ui.strong("目标窗口");
                ui.add_space(2.0);
                egui::Frame::default()
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.label("进程镜像名（不区分大小写）");
                        let before = self.settings.process.clone();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.process)
                                .hint_text("arknights.exe")
                                .desired_width(f32::INFINITY),
                        );
                        if before != self.settings.process {
                            self.shared
                                .settings
                                .lock()
                                .unwrap()
                                .clone_from(&self.settings);
                            self.settings.save();
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label("或窗口标题包含");
                            if ui.button("从进程列表选择…").clicked() {
                                self.processes = crate::process_list::list_window_processes();
                                self.process_list_open = !self.process_list_open;
                            }
                        });
                        if self.process_list_open {
                            egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                                for p in &self.processes {
                                    let label = format!("{}   —  {}", p.exe, p.title);
                                    if ui
                                        .selectable_label(
                                            self.settings.process.eq_ignore_ascii_case(&p.exe),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        self.settings.process = p.exe.clone();
                                        self.process_list_open = false;
                                        self.shared
                                            .settings
                                            .lock()
                                            .unwrap()
                                            .clone_from(&self.settings);
                                        self.settings.save();
                                    }
                                }
                            });
                        }
                        ui.add_space(2.0);
                        ui.label("窗口标题匹配（备用，包含即可）");
                        let before = self.settings.window_title.clone();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.window_title)
                                .hint_text("明日方舟")
                                .desired_width(f32::INFINITY),
                        );
                        if before != self.settings.window_title {
                            self.shared
                                .settings
                                .lock()
                                .unwrap()
                                .clone_from(&self.settings);
                            self.settings.save();
                        }
                    });

                ui.add_space(8.0);

                // ---- cursor group ----
                ui.strong("光标配置（大小与刷新率按文件记忆）");
                ui.add_space(2.0);
                egui::Frame::default()
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.label("光标文件（.cur / .ani / .ico / .png）");
                        // path on its own full-width row (won't be clipped by
                        // the right side panel)
                        let path_text = if self.settings.cursor_path.is_empty() {
                            "<未选择，使用内置默认光标>".to_string()
                        } else {
                            self.settings.cursor_path.clone()
                        };
                        ui.add(
                            egui::Label::new(egui::RichText::new(path_text).weak())
                                .wrap()
                                .truncate(),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("浏览文件…").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("光标文件", &["cur", "ani", "ico", "png"])
                                    .pick_file()
                                {
                                    self.settings.cursor_path =
                                        path.to_string_lossy().to_string();
                                    self.shared
                                        .settings
                                        .lock()
                                        .unwrap()
                                        .clone_from(&self.settings);
                                    self.settings.save();
                                    self.reload_profile_from_active_file();
                                }
                            }
                            // quick-switch dropdown over all added cursors
                            let options: Vec<String> =
                                self.settings.profiles.keys().cloned().collect();
                            let current = if self.settings.cursor_path.is_empty() {
                                "(内置默认)".to_string()
                            } else {
                                self.settings
                                    .cursor_path
                                    .rsplit(|c| c == char::from_u32(92).unwrap())
                                    .next()
                                    .unwrap_or(&self.settings.cursor_path)
                                    .to_string()
                            };
                            let before = self.settings.cursor_path.clone();
                            egui::ComboBox::from_id_salt("added_cursors")
                                .selected_text(format!("已添加: {current}"))
                                .width(160.0)
                                .show_ui(ui, |ui| {
                                    for opt in &options {
                                        let name = opt.rsplit(|c| c == char::from_u32(92).unwrap()).next().unwrap_or(opt);
                                        ui.selectable_value(
                                            &mut self.settings.cursor_path,
                                            opt.clone(),
                                            name,
                                        );
                                    }
                                });
                            if before != self.settings.cursor_path {
                                self.shared
                                    .settings
                                    .lock()
                                    .unwrap()
                                    .clone_from(&self.settings);
                                self.settings.save();
                                self.reload_profile_from_active_file();
                            }
                        });
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            ui.strong("大小");
                            let before = self.edit_size;
                            ui.add(
                                egui::Slider::new(&mut self.edit_size, 16..=128)
                                    .suffix(" px"),
                            );
                            if before != self.edit_size {
                                self.persist();
                            }
                        });

                        ui.horizontal(|ui| {
                            let follow_screen = self.edit_rate < 30;
                            let before = self.edit_rate;
                            if ui
                                .checkbox(&mut self.follow_screen, "跟随屏幕刷新率")
                                .changed()
                            {
                                self.edit_rate = if self.follow_screen {
                                    0
                                } else {
                                    self.screen_rate.max(60)
                                };
                            }
                            if self.edit_rate >= 30 {
                                ui.add(
                                    egui::Slider::new(&mut self.edit_rate, 30..=240)
                                        .suffix(" Hz"),
                                );
                            } else {
                                ui.label(format!("（当前 {} Hz）", self.screen_rate));
                            }
                            if before != self.edit_rate {
                                self.persist();
                            }
                        });
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::runtime::SHUTDOWN.store(true, Ordering::SeqCst);
    }
}
