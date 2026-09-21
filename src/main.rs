//! 三角洲行动 · 自动钓鱼（鼠标左键 + 声音回环检测）。
//!
//! GUI 采用 eframe/egui，深色玻璃拟态风格；后台由 engine 线程驱动。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod engine;
mod game;
mod input;
mod log;
mod studio;
mod theme;
mod updater;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui;
use eframe::egui::Color32;
use engine::State;
use log::Level;

fn main() -> eframe::Result {
    updater::clean_old_files();
    if !game::acquire_single_instance() {
        #[cfg(windows)]
        message_box("三角洲自动钓鱼", "程序已在运行，请勿重复启动。");
        #[cfg(not(windows))]
        eprintln!("程序已在运行");
        return Ok(());
    }

    let shared = Arc::new(Mutex::new(engine::Shared::new()));
    engine::spawn(shared.clone());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 780.0])
            .with_min_inner_size([380.0, 560.0])
            .with_title("三角洲行动 · 自动钓鱼")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "deltafishing",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, shared)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainTab {
    Fishing, // 🎣 自动钓鱼
    Studio,  // 🎙️ 声学工坊
}

struct App {
    shared: Arc<Mutex<engine::Shared>>,
    updater: updater::UpdaterHandle,
    current_tab: MainTab,
    studio: studio::StudioState,
    pinned: bool,
    pin_initialized: bool,
    show_settings: bool,
    sim_peak: f32,
    sim_peak_at: Instant,
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Mutex<engine::Shared>>) -> Self {
        theme::apply(&cc.egui_ctx);

        // 无边框 + 透明窗口：应用 Windows 亚克力背景与圆角。
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = cc.window_handle()
                && let RawWindowHandle::Win32(win) = handle.as_raw()
            {
                theme::apply_backdrop(win.hwnd.get() as isize);
            }
        }

        let upd = updater::UpdaterHandle::default();
        upd.check_async();

        let studio_state = studio::StudioState::default();

        Self {
            shared,
            updater: upd,
            current_tab: MainTab::Fishing,
            studio: studio_state,
            pinned: true,
            pin_initialized: false,
            show_settings: false,
            sim_peak: 0.0,
            sim_peak_at: std::time::Instant::now(),
        }
    }
}

fn level_color(l: Level) -> Color32 {
    match l {
        Level::Info => theme::TEXT_DIM,
        Level::Ok => theme::MINT,
        Level::Warn => theme::AMBER,
        Level::Error => theme::RED,
    }
}

/// 标题栏图标按钮：统一绘制底色/边框/交互，返回 (是否点击, 图标颜色)。
fn title_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    center: egui::Pos2,
    active: bool,
    tooltip: &str,
) -> (bool, Color32) {
    let rect = egui::Rect::from_center_size(center, egui::vec2(32.0, 26.0));
    let resp = ui.interact(rect, id, egui::Sense::click());
    resp.clone().on_hover_text(tooltip);

    let (bg, stroke) = if active {
        (
            Color32::from_rgba_unmultiplied(74, 222, 128, 30),
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(74, 222, 128, 110)),
        )
    } else if resp.hovered() {
        (
            Color32::from_white_alpha(18),
            egui::Stroke::new(1.0, Color32::from_white_alpha(60)),
        )
    } else {
        (
            Color32::from_white_alpha(6),
            egui::Stroke::new(1.0, theme::LINE),
        )
    };

    ui.painter().rect_filled(rect, egui::CornerRadius::same(7), bg);
    ui.painter().rect_stroke(rect, egui::CornerRadius::same(7), stroke, egui::StrokeKind::Middle);

    let fg = if active {
        theme::MINT
    } else if resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    (resp.clicked(), fg)
}

fn title_bar(
    ui: &mut egui::Ui,
    pinned: bool,
    settings_open: bool,
    has_update: bool,
) -> (bool, bool, bool) {
    let height = 44.0;
    let full = ui.available_rect_before_wrap();
    let rect = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), height));
    let id = ui.id().with("title_bar");

    ui.painter().rect_filled(rect, 0.0, theme::TITLEBAR);

    let drag = ui.interact(rect, id, egui::Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        let is_max = ui.input(|i| i.viewport().maximized).unwrap_or(false);
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
    }

    let cy = rect.center().y;

    // 左侧 Logo + 标题
    let dot_pos = egui::pos2(rect.min.x + 18.0, cy);
    ui.painter().circle_filled(dot_pos, 4.0, theme::MINT);
    ui.painter().circle_stroke(dot_pos, 7.0, egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(74, 222, 128, 60)));

    ui.painter().text(
        egui::pos2(rect.min.x + 32.0, cy),
        egui::Align2::LEFT_CENTER,
        "DELTA FISHING",
        egui::FontId::proportional(14.0),
        theme::TEXT,
    );

    // 版本号胶囊标签
    let ver_str = format!("v{}", env!("CARGO_PKG_VERSION"));
    let ver_pos = egui::pos2(rect.min.x + 148.0, cy);
    let ver_rect = egui::Rect::from_center_size(ver_pos + egui::vec2(16.0, 0.0), egui::vec2(46.0, 18.0));
    ui.painter().rect_filled(ver_rect, egui::CornerRadius::same(5), Color32::from_white_alpha(12));
    ui.painter().rect_stroke(ver_rect, egui::CornerRadius::same(5), egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Middle);
    ui.painter().text(
        ver_rect.center(),
        egui::Align2::CENTER_CENTER,
        ver_str,
        egui::FontId::monospace(10.5),
        theme::TEXT_MUTED,
    );

    // 右侧按钮组
    let close_c = egui::pos2(rect.right() - 24.0, cy);
    let pin_c = egui::pos2(rect.right() - 62.0, cy);
    let gear_c = egui::pos2(rect.right() - 100.0, cy);
    let gh_c = egui::pos2(rect.right() - 138.0, cy);

    let (close_clicked, close_fg) = {
        let r = egui::Rect::from_center_size(close_c, egui::vec2(30.0, 26.0));
        let resp = ui.interact(r, id.with("close"), egui::Sense::click());
        resp.clone().on_hover_text("退出程序");
        let (bg, stroke) = if resp.hovered() {
            (theme::RED, egui::Stroke::new(1.0, theme::RED))
        } else {
            (Color32::from_white_alpha(6), egui::Stroke::new(1.0, theme::LINE))
        };
        ui.painter().rect_filled(r, egui::CornerRadius::same(7), bg);
        ui.painter().rect_stroke(r, egui::CornerRadius::same(7), stroke, egui::StrokeKind::Middle);
        let fg = if resp.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
        (resp.clicked(), fg)
    };

    let (pin_clicked, pin_fg) = title_button(ui, id.with("pin"), pin_c, pinned, if pinned { "取消置顶" } else { "窗口置顶" });
    let (gear_clicked, gear_fg) = title_button(ui, id.with("settings"), gear_c, settings_open, "打开设置");
    let (gh_clicked, gh_fg) = title_button(ui, id.with("github"), gh_c, false, "访问 GitHub 仓库源码");

    // 绘制图标
    ui.painter().text(gh_c, egui::Align2::CENTER_CENTER, "GH", egui::FontId::monospace(11.0), gh_fg);

    let gc = gear_c;
    let gr = 3.8;
    ui.painter().circle_stroke(gc, gr, egui::Stroke::new(1.3, gear_fg));
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.0;
        let p1 = gc + egui::vec2(a.cos() * (gr + 1.4), a.sin() * (gr + 1.4));
        let p2 = gc + egui::vec2(a.cos() * (gr + 3.2), a.sin() * (gr + 3.2));
        ui.painter().line_segment([p1, p2], egui::Stroke::new(1.3, gear_fg));
    }
    if has_update {
        let dot_c = gear_c + egui::vec2(8.0, -8.0);
        ui.painter().circle_filled(dot_c, 3.5, theme::MINT);
        ui.painter().circle_stroke(dot_c, 5.5, egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(74, 222, 128, 80)));
    }

    let pc = pin_c;
    ui.painter().line_segment([egui::pos2(pc.x - 5.0, pc.y - 4.5), egui::pos2(pc.x + 4.0, pc.y - 4.5)], egui::Stroke::new(1.8, pin_fg));
    ui.painter().line_segment([egui::pos2(pc.x - 3.0, pc.y - 4.5), egui::pos2(pc.x - 2.0, pc.y - 1.0)], egui::Stroke::new(1.4, pin_fg));
    ui.painter().line_segment([egui::pos2(pc.x + 2.0, pc.y - 4.5), egui::pos2(pc.x + 1.0, pc.y - 1.0)], egui::Stroke::new(1.4, pin_fg));
    ui.painter().line_segment([egui::pos2(pc.x - 2.8, pc.y - 1.0), egui::pos2(pc.x + 1.8, pc.y - 1.0)], egui::Stroke::new(1.6, pin_fg));
    ui.painter().line_segment([egui::pos2(pc.x - 0.5, pc.y - 1.0), egui::pos2(pc.x + 4.0, pc.y + 5.0)], egui::Stroke::new(1.6, pin_fg));

    let r = 4.5;
    ui.painter().line_segment([close_c + egui::vec2(-r, -r), close_c + egui::vec2(r, r)], egui::Stroke::new(1.4, close_fg));
    ui.painter().line_segment([close_c + egui::vec2(-r, r), close_c + egui::vec2(r, -r)], egui::Stroke::new(1.4, close_fg));

    if close_clicked {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }

    ui.painter().line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0, theme::LINE),
    );

    ui.advance_cursor_after_rect(rect);
    (pin_clicked, gear_clicked, gh_clicked)
}

/// 指标条：第一行标签（左）+ 数值（右），第二行全宽圆角轨道与填充。
fn glow_meter(ui: &mut egui::Ui, label: &str, value: f32, glow_color: Color32, threshold_pct: Option<f32>) {
    let v = value.clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(12.5).color(theme::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let is_hit = threshold_pct.map(|th| (v * 100.0) >= th).unwrap_or(false);
            let num_color = if is_hit { theme::MINT } else { glow_color };
            ui.label(
                egui::RichText::new(format!("{:.0}%", v * 100.0))
                    .size(13.0)
                    .strong()
                    .color(num_color)
                    .monospace(),
            );
        });
    });
    ui.add_space(2.0);

    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 7.0), egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, egui::CornerRadius::same(3), Color32::from_black_alpha(100));
    painter.rect_stroke(rect, egui::CornerRadius::same(3), egui::Stroke::new(1.0, Color32::from_white_alpha(12)), egui::StrokeKind::Middle);

    if let Some(th) = threshold_pct {
        let th_x = rect.left() + rect.width() * (th / 100.0).clamp(0.0, 1.0);
        painter.line_segment(
            [egui::pos2(th_x, rect.top() - 1.0), egui::pos2(th_x, rect.bottom() + 1.0)],
            egui::Stroke::new(1.2, Color32::from_white_alpha(60)),
        );
    }

    if v > 0.005 {
        let fill_w = (rect.width() * v).max(4.0);
        let fill_rect = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
        let glow_rect = fill_rect.expand(1.5);
        painter.rect_filled(
            glow_rect,
            egui::CornerRadius::same(4),
            Color32::from_rgba_unmultiplied(glow_color.r(), glow_color.g(), glow_color.b(), 35),
        );
        painter.rect_filled(fill_rect, egui::CornerRadius::same(3), glow_color);
    }
}

/// 立体声方位雷达条（直观可视化正前方锁定与侧面他人过滤）。
fn stereo_pan_radar(ui: &mut egui::Ui, pan_db: f32, max_pan_db: f32, level_db: f32) {
    let has_sound = level_db > -80.0;
    let clamped_pan = pan_db.clamp(-12.0, 12.0);
    let is_centered = has_sound && pan_db.abs() <= max_pan_db;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("声像方位雷达").size(12.0).color(theme::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (status_str, status_color) = if !has_sound {
                ("等待声音...".to_string(), theme::TEXT_MUTED)
            } else if is_centered {
                ("🎯 正前方准星锁定".to_string(), theme::MINT)
            } else if pan_db > 0.0 {
                (format!("⚠ 偏左 {:.1}dB (他人)", pan_db), theme::AMBER)
            } else {
                (format!("⚠ 偏右 {:.1}dB (他人)", pan_db.abs()), theme::AMBER)
            };
            ui.label(egui::RichText::new(status_str).size(11.5).strong().color(status_color));
        });
    });
    ui.add_space(2.0);

    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, egui::CornerRadius::same(5), Color32::from_black_alpha(90));
    painter.rect_stroke(rect, egui::CornerRadius::same(5), egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Middle);

    let center_x = rect.center().x;
    let half_w = rect.width() / 2.0;

    // 正前方安全锁定区（绿色微透半透明区域）
    let zone_half_w = (half_w * (max_pan_db / 12.0)).min(half_w);
    let safe_rect = egui::Rect::from_min_max(
        egui::pos2(center_x - zone_half_w, rect.top() + 1.0),
        egui::pos2(center_x + zone_half_w, rect.bottom() - 1.0),
    );
    painter.rect_filled(safe_rect, egui::CornerRadius::same(3), Color32::from_rgba_unmultiplied(74, 222, 128, 25));

    // 中央正前方准星线 (0 dB)
    painter.line_segment(
        [egui::pos2(center_x, rect.top()), egui::pos2(center_x, rect.bottom())],
        egui::Stroke::new(1.0, Color32::from_white_alpha(50)),
    );

    // 游标位置映射（pan_db = L - R，正数偏左，负数偏右）
    let cursor_ratio = (-clamped_pan / 12.0).clamp(-1.0, 1.0);
    let cursor_x = center_x + cursor_ratio * half_w;

    let cursor_color = if !has_sound {
        theme::TEXT_MUTED
    } else if is_centered {
        theme::MINT
    } else {
        theme::AMBER
    };

    let cursor_center = egui::pos2(cursor_x, rect.center().y);
    if has_sound {
        painter.circle_filled(
            cursor_center,
            5.5,
            Color32::from_rgba_unmultiplied(cursor_color.r(), cursor_color.g(), cursor_color.b(), 50),
        );
    }
    painter.circle_filled(cursor_center, 3.5, cursor_color);
    painter.circle_stroke(cursor_center, 3.5, egui::Stroke::new(1.0, Color32::WHITE));

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("◀ 左侧侧翼").size(10.0).color(theme::TEXT_MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("右侧侧翼 ▶").size(10.0).color(theme::TEXT_MUTED));
            let zone_text = format!("准星锁定安全角 (±{:.0}dB)", max_pan_db);
            ui.label(egui::RichText::new(zone_text).size(10.0).color(theme::TEXT_DIM));
        });
    });
}

fn stat_card(ui: &mut egui::Ui, width: f32, icon: &str, label: &str, value: u64, color: Color32) {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(width - 20.0, 52.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icon).size(11.0));
                        ui.label(egui::RichText::new(label).size(11.5).color(theme::TEXT_DIM));
                    });
                    ui.add_space(1.0);
                    ui.label(
                        egui::RichText::new(value.to_string())
                            .size(22.0)
                            .strong()
                            .color(color)
                            .monospace(),
                    );
                },
            );
        });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let update_status = self.updater.get_status();
        // ---- 快照共享状态（克隆后释放锁） ----
        let (running, paused, state, stats, last_sim, last_level_db, last_pan_db, audio_level, foreground_exe,
            devices, logs) = {
            let s = self.shared.lock().unwrap();
            (
                s.running,
                s.paused,
                s.state,
                s.stats,
                s.last_sim,
                s.last_level_db,
                s.last_pan_db,
                s.audio_level,
                s.foreground_exe.clone(),
                s.devices.clone(),
                s.log.iter().cloned().collect::<Vec<_>>(),
            )
        };

        // 每帧先衰减上一次显示值，再与当前检测值比较，避免历史峰值阻止刷新。
        let now = std::time::Instant::now();
        self.sim_peak = if running && !paused {
            display_similarity(self.sim_peak, last_sim, now.duration_since(self.sim_peak_at).as_secs_f32())
        } else {
            0.0
        };
        self.sim_peak_at = now;
        let sim_display = self.sim_peak;

        let mut cfg = self.shared.lock().unwrap().config.clone();
        let mut cfg_changed = false;
        let mut request_toggle = false;
        let mut request_clear = false;
        let mut request_export = false;
        let pinned = self.pinned;
        let mut pin_toggled = false;
        let settings_open = self.show_settings;
        let mut settings_toggled = false;

        if !self.pin_initialized {
            self.pin_initialized = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::viewport::WindowLevel::AlwaysOnTop,
            ));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(ui.style())
                    .fill(theme::GLASS)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                let has_update = matches!(update_status, updater::UpdateStatus::Available(_));
                let (pin_click, gear_click, gh_click) = title_bar(ui, pinned, settings_open, has_update);
                if gh_click {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(updater::GITHUB_REPO));
                }

                // ---- 顶部主导航 Tab 栏 ----
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    let fishing_active = self.current_tab == MainTab::Fishing;
                    let studio_active = self.current_tab == MainTab::Studio;

                    let (bg_f, fg_f) = if fishing_active {
                        (Color32::from_rgba_unmultiplied(74, 222, 128, 30), theme::MINT)
                    } else {
                        (Color32::from_white_alpha(8), theme::TEXT_DIM)
                    };
                    let btn_f = egui::Button::new(egui::RichText::new("🎣  自动钓鱼挂机").size(13.0).strong().color(fg_f))
                        .fill(bg_f)
                        .stroke(egui::Stroke::new(1.0, if fishing_active { theme::MINT } else { theme::LINE }))
                        .corner_radius(egui::CornerRadius::same(7));
                    if ui.add(btn_f).clicked() && self.current_tab != MainTab::Fishing {
                        self.current_tab = MainTab::Fishing;
                        // 自动切换为钓鱼紧凑窄屏悬浮窗模式 (450 x 780)
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(450.0, 780.0)));
                    }

                    let (bg_s, fg_s) = if studio_active {
                        (Color32::from_rgba_unmultiplied(56, 189, 248, 30), theme::CYAN)
                    } else {
                        (Color32::from_white_alpha(8), theme::TEXT_DIM)
                    };
                    let btn_s = egui::Button::new(egui::RichText::new("🎙️  声学标定工坊").size(13.0).strong().color(fg_s))
                        .fill(bg_s)
                        .stroke(egui::Stroke::new(1.0, if studio_active { theme::CYAN } else { theme::LINE }))
                        .corner_radius(egui::CornerRadius::same(7));
                    if ui.add(btn_s).clicked() && self.current_tab != MainTab::Studio {
                        self.current_tab = MainTab::Studio;
                        // 自动切换为声学工坊宽屏大图模式 (860 x 840)
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(860.0, 840.0)));
                        self.studio.refresh_available_recordings();
                    }
                });
                ui.add_space(4.0);

                match self.current_tab {
                    MainTab::Fishing => {
                pin_toggled = pin_click;
                settings_toggled = gear_click;

                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(16, 14))
                            .show(ui, |ui| {
                                // ---- 状态徽章行 ----
                                ui.horizontal(|ui| {
                                    let (dot, txt, txt_color) = if !running {
                                        (theme::TEXT_DIM, "已停止", theme::TEXT_DIM)
                                    } else if paused {
                                        (theme::AMBER, "已暂停", theme::AMBER)
                                    } else {
                                        (theme::MINT, "运行中", theme::MINT)
                                    };
                                    ui.colored_label(dot, "●");
                                    ui.label(
                                        egui::RichText::new(txt).color(txt_color).strong().size(14.0),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if false {
                                                ui.colored_label(theme::AMBER, "游戏未前台");
                                            }
                                        },
                                    );
                                });

                                ui.add_space(12.0);

                                // ---- 主状态卡 ----
                                egui::Frame::new()
                                    .fill(theme::CARD)
                                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                                    .corner_radius(egui::CornerRadius::same(12))
                                    .inner_margin(egui::Margin::same(14))
                                    .show(ui, |ui| {
                                        let state_color =
                            if paused || !running { theme::AMBER } else { theme::MINT };
                                        ui.label(
                                            egui::RichText::new(if paused {
                                                "已暂停".to_string()
                                            } else {
                                                state.label().to_string()
                                            })
                                            .size(22.0)
                                            .strong()
                                            .color(state_color),
                                        );
                                        ui.add_space(10.0);

                                        glow_meter(ui, "咬钩相似度", sim_display, theme::CYAN, Some(cfg.bite_threshold));
                                        ui.add_space(10.0);
                                        glow_meter(ui, "输出音量", audio_level, theme::BLUE, None);
                                        ui.add_space(12.0);

                                        // 立体声方位雷达
                                        stereo_pan_radar(ui, last_pan_db, cfg.pan_max_db, last_level_db);
                                        ui.add_space(10.0);

                                        // 细分隔线
                                        let (sep, _) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 1.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().rect_filled(
                                            sep,
                                            0.0,
                                            Color32::from_white_alpha(12),
                                        );
                                        ui.add_space(10.0);

                                        // 前台状态行：圆点 + 状态，右侧进程名
                                        ui.horizontal(|ui| {
                                            let (dot, txt) = (theme::MINT, "游戏前台");
                                            let (dot_rect, _) = ui.allocate_exact_size(
                                                egui::vec2(8.0, 14.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter()
                                                .circle_filled(dot_rect.center(), 3.0, dot);
                                            ui.label(
                                                egui::RichText::new(txt)
                                                    .size(12.0)
                                                    .color(theme::TEXT),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let name = if foreground_exe.is_empty() {
                                                        "—".to_string()
                                                    } else {
                                                        foreground_exe.clone()
                                                    };
                                                    ui.label(
                                                        egui::RichText::new(name)
                                                            .size(11.0)
                                                            .color(theme::TEXT_DIM),
                                                    );
                                                },
                                            );
                                        });
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "门限: 相似度≥{:.0}% · 电平≥{:.0}dB · 声像≤±{:.1}dB",
                                                    cfg.bite_threshold, cfg.level_min_db, cfg.pan_max_db
                                                ))
                                                .size(10.5)
                                                .color(theme::TEXT_DIM),
                                            );
                                        });
                                    });

                                ui.add_space(12.0);

                                // ---- 统计（四宫格等宽） ----
                                ui.horizontal(|ui| {
                                    let sp = ui.spacing().item_spacing.x;
                                    let w = (ui.available_width() - sp * 3.0) / 4.0;
                                    stat_card(ui, w, "🎣", "抛竿", stats.casts, theme::CYAN);
                                    stat_card(ui, w, "⚡", "咬钩", stats.bites, theme::AMBER);
                                    stat_card(ui, w, "🐟", "钓获", stats.catches, theme::MINT);
                                    stat_card(ui, w, "💨", "脱钩", stats.misses, theme::RED);
                                });

                                ui.add_space(14.0);

                                // ---- 开始/停止 ----
                                if running {
                                    let btn = egui::Button::new(
                                        egui::RichText::new("停止")
                                            .size(16.0)
                                            .strong()
                                            .color(theme::RED),
                                    )
                                    .fill(Color32::from_rgba_unmultiplied(0x46, 0x20, 0x20, 240))
                                    .corner_radius(egui::CornerRadius::same(10))
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        Color32::from_rgb(0x6a, 0x30, 0x30),
                                    ));
                                    if ui.add_sized([ui.available_width(), 42.0], btn).clicked() {
                                        request_toggle = true;
                                    }
                                } else {
                                    let btn = egui::Button::new(
                                        egui::RichText::new("开始钓鱼")
                                            .size(17.0)
                                            .strong()
                                            .color(Color32::from_rgb(8, 28, 14)),
                                    )
                                    .fill(theme::MINT)
                                    .corner_radius(egui::CornerRadius::same(10));
                                    if ui.add_sized([ui.available_width(), 42.0], btn).clicked() {
                                        request_toggle = true;
                                    }
                                }
                                ui.add_space(6.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new("F8 暂停 · F9 退出 · 仅游戏前台时动作")
                                            .size(10.5)
                                            .color(theme::TEXT_DIM),
                                    );
                                });

                                ui.add_space(14.0);

                                // ---- 日志 ----
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("日志").strong().color(theme::TEXT),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("导出").clicked() {
                                                request_export = true;
                                            }
                                            if ui.small_button("清空").clicked() {
                                                request_clear = true;
                                            }
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                egui::Frame::new()
                                    .fill(Color32::from_black_alpha(70))
                                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                                    .corner_radius(egui::CornerRadius::same(10))
                                    .inner_margin(egui::Margin::same(8))
                                    .show(ui, |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt("log-scroll")
                                            .max_height(200.0)
                                            .stick_to_bottom(true)
                                            .show(ui, |ui| {
                                                if logs.is_empty() {
                                                    ui.label(
                                                        egui::RichText::new("暂无日志")
                                                            .color(theme::TEXT_DIM)
                                                            .size(12.0),
                                                    );
                                                }
                                                for e in &logs {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(&e.time)
                                                                .color(theme::TEXT_DIM)
                                                                .monospace()
                                                                .size(11.0),
                                                        );
                                                        ui.label(
                                                            egui::RichText::new(&e.text)
                                                                .color(level_color(e.level))
                                                                .size(12.0),
                                                        );
                                                    });
                                                }
                                            });
                                    });

                                ui.add_space(12.0);
                            });
                    });
                    }
                    MainTab::Studio => {
                        let dev = cfg.device_name.clone();
                        studio::render_studio_panel(ui, &mut self.studio, &mut cfg, &mut cfg_changed, &dev);
                    }
                }

                // 自由拉伸边框与右下角把手
                handle_window_resize(ui);
            });

        // ---- 设置弹窗 (模块化分类卡片设计) ----
        if settings_toggled {
            self.show_settings = !self.show_settings;
        }
        let mut settings_open = self.show_settings;
        let mut close_settings = false;

        egui::Window::new(egui::RichText::new("设置中心").strong().color(theme::TEXT))
            .open(&mut settings_open)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .min_width(430.0)
            .max_width(450.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(ui.style())
                    .fill(Color32::from_rgb(16, 26, 20))
                    .stroke(egui::Stroke::new(1.0, theme::LINE_LIGHT))
                    .corner_radius(egui::CornerRadius::same(16))
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("⚙  参数配置中心").strong().size(15.5).color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new(" ✕ ").strong().size(13.0).color(theme::TEXT_DIM)).clicked() {
                            close_settings = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .max_height(530.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!running, |ui| {
                            // ---- 模块 1：游戏与硬件关联 ----
                            egui::Frame::new()
                                .fill(theme::CARD)
                                .stroke(egui::Stroke::new(1.0, theme::LINE))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("🎮 游戏与音频设备").strong().size(13.0).color(theme::CYAN));
                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        ui.label("游戏进程:");
                                        if ui.add(egui::TextEdit::singleline(&mut cfg.game_exe).desired_width(220.0)).changed() {
                                            cfg_changed = true;
                                        }
                                    });

                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label("音频回环:");
                                        let mut sel: usize = if cfg.device_name.is_empty() {
                                            0
                                        } else {
                                            devices.iter().position(|d| *d == cfg.device_name).map(|i| i + 1).unwrap_or(0)
                                        };
                                        let sel_text = if sel == 0 {
                                            "系统默认输出设备".to_string()
                                        } else {
                                            devices.get(sel - 1).cloned().unwrap_or_default()
                                        };
                                        egui::ComboBox::from_id_salt("audio-device-sel")
                                            .width(ui.available_width())
                                            .selected_text(sel_text)
                                            .show_ui(ui, |ui| {
                                                if ui.selectable_value(&mut sel, 0usize, "系统默认输出设备").changed() {
                                                    cfg.device_name = String::new();
                                                    cfg_changed = true;
                                                }
                                                for (i, d) in devices.iter().enumerate() {
                                                    if ui.selectable_value(&mut sel, i + 1, d.as_str()).changed() {
                                                        cfg.device_name = d.clone();
                                                        cfg_changed = true;
                                                    }
                                                }
                                            });
                                    });
                                });

                            ui.add_space(10.0);

                            // ---- 模块 2：声学抗干扰三层闸门 ----
                            egui::Frame::new()
                                .fill(theme::CARD)
                                .stroke(egui::Stroke::new(1.0, theme::LINE))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("🎯 声学抗干扰三层闸门").strong().size(13.0).color(theme::MINT));
                                    ui.add_space(2.0);
                                    ui.label(egui::RichText::new("通过“相似度 + 距离电平 + 空间方位”彻底过滤他人咬钩与枪炮噪音").size(11.0).color(theme::TEXT_MUTED));
                                    ui.add_space(6.0);

                                    if ui
                                        .add(egui::Slider::new(&mut cfg.bite_threshold, 0.0..=100.0).text("1. 咬钩相似度").suffix(" %"))
                                        .on_hover_text("波形与包络 ZNCC 归一化互相关分数")
                                        .changed()
                                    {
                                        cfg_changed = true;
                                    }

                                    if ui
                                        .add(egui::Slider::new(&mut cfg.level_min_db, -50.0..=-10.0).text("2. 最小电平门限").suffix(" dB"))
                                        .on_hover_text("过滤远处其他玩家的微弱咬钩声，建议保持 -28 dB 左右")
                                        .changed()
                                    {
                                        cfg_changed = true;
                                    }

                                    if ui
                                        .add(egui::Slider::new(&mut cfg.pan_max_db, 1.0..=15.0).text("3. 最大声像偏离").suffix(" dB"))
                                        .on_hover_text("左右声道分贝差超出此值判定为侧面他人，建议 5.0 dB")
                                        .changed()
                                    {
                                        cfg_changed = true;
                                    }
                                });

                            ui.add_space(10.0);

                            // ---- 模块 3：动作与时序增强 ----
                            egui::Frame::new()
                                .fill(theme::CARD)
                                .stroke(egui::Stroke::new(1.0, theme::LINE))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("⚡ 自动化连招与容错").strong().size(13.0).color(theme::AMBER));
                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        if ui.checkbox(&mut cfg.hold_rmb, "右键缩放聚焦").on_hover_text("利用游戏音频引擎的定向衰减主动过滤旁人杂音").changed() {
                                            cfg_changed = true;
                                        }
                                        if ui.checkbox(&mut cfg.skip_anim, "打断展示鱼动画").on_hover_text("刺鱼后约1.2秒轻点左键跳过展示鱼，大幅提速").changed() {
                                            cfg_changed = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if ui.checkbox(&mut cfg.reset_on_timeout, "超时 3→6 切刀重置").on_hover_text("单轮超时未咬钩时自动切刀切竿，强制恢复人物就绪状态").changed() {
                                            cfg_changed = true;
                                        }
                                        if ui.checkbox(&mut cfg.double_cast, "抛竿双击").changed() {
                                            cfg_changed = true;
                                        }
                                    });

                                    ui.add_space(6.0);
                                    egui::Grid::new("cfg-timing-grid")
                                        .num_columns(4)
                                        .spacing([12.0, 6.0])
                                        .show(ui, |ui| {
                                            ui.label("抛竿等待:");
                                            if ui.add(egui::DragValue::new(&mut cfg.cast_delay).speed(0.1).suffix(" s")).changed() {
                                                cfg_changed = true;
                                            }
                                            ui.label("刺鱼反应:");
                                            if ui.add(egui::DragValue::new(&mut cfg.strike_delay).speed(0.05).suffix(" s")).changed() {
                                                cfg_changed = true;
                                            }
                                            ui.end_row();

                                            ui.label("收竿等待:");
                                            if ui.add(egui::DragValue::new(&mut cfg.reel_wait_min).speed(0.1).suffix(" s")).changed() {
                                                cfg_changed = true;
                                            }
                                            ui.label("— 上限:");
                                            if ui.add(egui::DragValue::new(&mut cfg.reel_wait_max).speed(0.1).suffix(" s")).changed() {
                                                cfg_changed = true;
                                            }
                                            ui.end_row();

                                            ui.label("单轮超时:");
                                            if ui.add(egui::DragValue::new(&mut cfg.round_timeout).speed(0.5).suffix(" s")).changed() {
                                                cfg_changed = true;
                                            }
                                            ui.label("按键保持:");
                                            if ui.add(egui::DragValue::new(&mut cfg.click_hold).speed(0.01).suffix(" s")).changed() {
                                                cfg_changed = true;
                                            }
                                            ui.end_row();
                                        });
                                });

                            ui.add_space(10.0);

                            // ---- 模块 4：关于与自动更新 ----
                            egui::Frame::new()
                                .fill(theme::CARD)
                                .stroke(egui::Stroke::new(1.0, theme::LINE))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("🌐 关于与在线自动更新").strong().size(13.0).color(theme::TEXT));
                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("GitHub 源码:").size(12.0).color(theme::TEXT_DIM));
                                        if ui.hyperlink_to("olwater/DeltaFishing", updater::GITHUB_REPO).clicked() {}
                                    });
                                    ui.add_space(4.0);

                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(format!("当前版本: v{}", env!("CARGO_PKG_VERSION"))).size(12.0).color(theme::TEXT_DIM));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let btn_check = egui::Button::new(
                                                egui::RichText::new("检查更新").size(11.5).color(theme::TEXT)
                                            )
                                            .fill(Color32::from_white_alpha(15))
                                            .corner_radius(egui::CornerRadius::same(6));
                                            if ui.add(btn_check).clicked() {
                                                self.updater.check_async();
                                            }
                                        });
                                    });

                                    ui.add_space(4.0);

                                    match &update_status {
                                        updater::UpdateStatus::Checking => {
                                            ui.horizontal(|ui| {
                                                ui.spinner();
                                                ui.label(egui::RichText::new("正在连接 GitHub 检测最新版本...").size(11.0).color(theme::TEXT_DIM));
                                            });
                                        }
                                        updater::UpdateStatus::UpToDate => {
                                            ui.label(egui::RichText::new("✓ 已是最新版本 (v".to_string() + env!("CARGO_PKG_VERSION") + ")").size(11.5).color(theme::MINT));
                                        }
                                        updater::UpdateStatus::Available(info) => {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(format!("★ 发现新版 v{}！", info.latest_version)).size(12.0).color(theme::MINT).strong());
                                                if let Some(dl) = &info.download_url {
                                                    if ui.small_button("一键自动更新").clicked() {
                                                        self.updater.download_and_install_async(dl.clone());
                                                    }
                                                }
                                            });
                                        }
                                        updater::UpdateStatus::Downloading { progress } => {
                                            ui.label(egui::RichText::new(format!("正在下载更新: {:.0}%", progress * 100.0)).size(11.5).color(theme::CYAN));
                                        }
                                        updater::UpdateStatus::ReadyToRestart => {
                                            ui.label(egui::RichText::new("更新就绪，正在自动重启应用...").size(11.5).color(theme::MINT).strong());
                                        }
                                        updater::UpdateStatus::Failed(err) => {
                                            ui.label(egui::RichText::new(format!("检测失败: {err}")).size(11.0).color(theme::RED));
                                        }
                                        updater::UpdateStatus::Idle => {}
                                    }
                                });
                        });
                    });
            });

        self.show_settings = settings_open && !close_settings;

        // ---- 置顶切换 ----
        if pin_toggled {
            self.pinned = !self.pinned;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                if self.pinned {
                    egui::viewport::WindowLevel::AlwaysOnTop
                } else {
                    egui::viewport::WindowLevel::Normal
                },
            ));
        }

        // ---- 应用变更 ----
        {
            let mut s = self.shared.lock().unwrap();
            if request_toggle {
                s.running = !s.running;
                if !s.running {
                    s.paused = false;
                    s.state = State::Stopped;
                }
            }
            if cfg_changed {
                s.config = cfg;
                if let Err(e) = s.config.save() {
                    s.log.error(format!("保存配置失败: {e}"));
                }
            }
            if request_clear {
                s.log.clear();
            }
            if request_export {
                export_log(&mut s.log);
            }
        }

        ui.ctx().request_repaint();
    }
}


/// 无边框窗口多方向与右下角把手自由拖拽拉伸。
fn handle_window_resize(ui: &egui::Ui) {
    let screen = ui.max_rect();
    let border = 7.0;
    let grip_size = 18.0;

    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    let mouse_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));

    // 右下角三斜杠拉伸把手高亮绘制
    let p = ui.painter();
    for offset in [4.0, 8.0, 12.0] {
        p.line_segment(
            [
                egui::pos2(screen.max.x - offset, screen.max.y - 2.0),
                egui::pos2(screen.max.x - 2.0, screen.max.y - offset),
            ],
            egui::Stroke::new(1.3, Color32::from_white_alpha(50)),
        );
    }

    if let Some(pos) = hover_pos {
        let is_corner = pos.x >= screen.max.x - grip_size && pos.y >= screen.max.y - grip_size;
        let is_right = pos.x >= screen.max.x - border && pos.y >= screen.min.y + 42.0;
        let is_bottom = pos.y >= screen.max.y - border;
        let is_left = pos.x <= screen.min.x + border && pos.y >= screen.min.y + 42.0;

        let mut resize_dir = None;
        let mut cursor_icon = None;

        if is_corner {
            resize_dir = Some(egui::viewport::ResizeDirection::SouthEast);
            cursor_icon = Some(egui::CursorIcon::ResizeSouthEast);
        } else if is_right {
            resize_dir = Some(egui::viewport::ResizeDirection::East);
            cursor_icon = Some(egui::CursorIcon::ResizeEast);
        } else if is_bottom {
            resize_dir = Some(egui::viewport::ResizeDirection::South);
            cursor_icon = Some(egui::CursorIcon::ResizeSouth);
        } else if is_left {
            resize_dir = Some(egui::viewport::ResizeDirection::West);
            cursor_icon = Some(egui::CursorIcon::ResizeWest);
        }

        if let Some(icon) = cursor_icon {
            ui.ctx().set_cursor_icon(icon);
        }

        if mouse_down {
            if let Some(dir) = resize_dir {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
            }
        }
    }
}

fn export_log(log: &mut log::LogBuffer) {
    let path = config::Config::config_path().with_file_name("fishing.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, log.export()) {
        Ok(_) => log.ok(format!("日志已导出：{}", path.display())),
        Err(e) => log.error(format!("日志导出失败：{e}")),
    }
}

#[cfg(windows)]
fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;

    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            0x40, // MB_ICONINFORMATION | MB_OK
        );
    }
}

/// 峰值在约 0.45 秒内降至一半，但始终不低于当前检测值。
fn display_similarity(previous: f32, current: f32, elapsed: f32) -> f32 {
    (previous * 0.5f32.powf(elapsed / 0.45)).max(current).clamp(0.0, 1.0)
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn lower_bites_remain_visible_after_old_peak() {
        let mut shown = 0.95;
        for _ in 0..12000 {
            shown = display_similarity(shown, 0.2, 0.016);
        }
        assert!((shown - 0.2).abs() < 0.001);
        assert_eq!(display_similarity(shown, 0.7, 0.016), 0.7);
    }

    #[test]
    fn silence_decays_display_without_changing_detection() {
        assert!((display_similarity(0.8, 0.0, 1.5) - 0.4).abs() < 0.001);
    }
}
