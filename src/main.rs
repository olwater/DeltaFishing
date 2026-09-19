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
mod theme;

use std::sync::{Arc, Mutex};

use eframe::egui;
use eframe::egui::Color32;
use engine::State;
use log::Level;

fn main() -> eframe::Result {
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
            .with_inner_size([460.0, 800.0])
            .with_min_inner_size([380.0, 600.0])
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

struct App {
    shared: Arc<Mutex<engine::Shared>>,
    /// 窗口是否置顶。
    pinned: bool,
    pin_initialized: bool,
    /// 设置弹窗是否打开。
    show_settings: bool,
    /// 相似度峰值保持（冲高后缓慢回落）。
    sim_peak: f32,
    sim_peak_at: std::time::Instant,
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

        Self {
            shared,
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
    let rect = egui::Rect::from_center_size(center, egui::vec2(36.0, 26.0));
    let resp = ui.interact(rect, id, egui::Sense::click());
    resp.clone().on_hover_text(tooltip);
    // 底色/边框：激活态 mint 高亮，悬停微亮，平时暗玻璃。
    let (bg, stroke) = if active {
        (
            Color32::from_rgba_unmultiplied(0xbc, 0xf7, 0x9b, 36),
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xbc, 0xf7, 0x9b, 110)),
        )
    } else if resp.hovered() {
        (
            Color32::from_white_alpha(20),
            egui::Stroke::new(1.0, Color32::from_white_alpha(60)),
        )
    } else {
        (
            Color32::from_white_alpha(8),
            egui::Stroke::new(1.0, theme::LINE),
        )
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(8), bg);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(8),
        stroke,
        egui::StrokeKind::Middle,
    );
    let fg = if active {
        theme::MINT
    } else if resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    (resp.clicked(), fg)
}

/// 顶部自定义标题栏：可拖动窗口、设置/置顶按钮、关闭按钮。
///
/// 返回 (置顶按钮被点击, 设置按钮被点击)。
fn title_bar(ui: &mut egui::Ui, pinned: bool, settings_open: bool) -> (bool, bool) {
    let height = 42.0;
    let full = ui.available_rect_before_wrap();
    let rect = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), height));
    let id = ui.id().with("title_bar");

    ui.painter().rect_filled(rect, 0.0, theme::TITLEBAR);

    let drag = ui.interact(rect, id, egui::Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    ui.painter().text(
        rect.left_center() + egui::vec2(16.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "三角洲 · 自动钓鱼",
        egui::FontId::proportional(15.0),
        theme::TEXT,
    );

    // ---- 设置按钮（齿轮） ----
    let gear_center = egui::pos2(rect.right() - 98.0, rect.center().y);
    let (gear_clicked, gear_fg) =
        title_button(ui, id.with("settings"), gear_center, settings_open, "打开设置");
    let gc = gear_center;
    let gr = 4.0;
    ui.painter()
        .circle_stroke(gc, gr, egui::Stroke::new(1.4, gear_fg));
    ui.painter().circle_filled(gc, 1.4, gear_fg);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.0;
        let p1 = gc + egui::vec2(a.cos() * (gr + 1.6), a.sin() * (gr + 1.6));
        let p2 = gc + egui::vec2(a.cos() * (gr + 3.4), a.sin() * (gr + 3.4));
        ui.painter()
            .line_segment([p1, p2], egui::Stroke::new(1.4, gear_fg));
    }

    // ---- 置顶按钮（图钉） ----
    let pin_center = egui::pos2(rect.right() - 58.0, rect.center().y);
    let (pin_clicked, pin_fg) = title_button(ui, id.with("pin"), pin_center, pinned, if pinned { "取消置顶" } else { "置顶窗口" });
    let pc = pin_center;
    // 标准图钉：横向针帽、短颈、斜向针杆和尖端。
    ui.painter().line_segment(
        [egui::pos2(pc.x - 5.5, pc.y - 5.0), egui::pos2(pc.x + 4.0, pc.y - 5.0)],
        egui::Stroke::new(1.8, pin_fg),
    );
    ui.painter().line_segment(
        [egui::pos2(pc.x - 3.5, pc.y - 5.0), egui::pos2(pc.x - 2.5, pc.y - 1.0)],
        egui::Stroke::new(1.4, pin_fg),
    );
    ui.painter().line_segment(
        [egui::pos2(pc.x + 2.0, pc.y - 5.0), egui::pos2(pc.x + 1.0, pc.y - 1.0)],
        egui::Stroke::new(1.4, pin_fg),
    );
    ui.painter().line_segment(
        [egui::pos2(pc.x - 3.0, pc.y - 1.0), egui::pos2(pc.x + 1.5, pc.y - 1.0)],
        egui::Stroke::new(1.6, pin_fg),
    );
    ui.painter().line_segment(
        [egui::pos2(pc.x - 0.8, pc.y - 1.0), egui::pos2(pc.x + 4.5, pc.y + 5.0)],
        egui::Stroke::new(1.7, pin_fg),
    );

    // ---- 关闭按钮 ----
    let close_center = egui::pos2(rect.right() - 22.0, rect.center().y);
    let close_rect = egui::Rect::from_center_size(close_center, egui::vec2(30.0, 28.0));
    let close = ui.interact(close_rect, id.with("close"), egui::Sense::click());
    close.clone().on_hover_text("退出程序");
    close.clone().on_hover_text("退出程序");
    if close.hovered() {
        ui.painter()
            .rect_filled(close_rect, egui::CornerRadius::same(8), theme::RED);
    }
    let (cx, cy) = (close_rect.center().x, close_rect.center().y);
    let r = 5.0;
    let xc = if close.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
    ui.painter().line_segment(
        [egui::pos2(cx - r, cy - r), egui::pos2(cx + r, cy + r)],
        egui::Stroke::new(1.5, xc),
    );
    ui.painter().line_segment(
        [egui::pos2(cx - r, cy + r), egui::pos2(cx + r, cy - r)],
        egui::Stroke::new(1.5, xc),
    );
    if close.clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }

    ui.painter().line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0, theme::LINE),
    );

    ui.advance_cursor_after_rect(rect);
    (pin_clicked, gear_clicked)
}

/// 指标条：第一行标签（左）+ 数值（右），第二行全宽圆角轨道与填充。
fn meter(ui: &mut egui::Ui, label: &str, value: f32, color: Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).size(12.5).color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{:.0}%", value * 100.0))
                        .size(13.0)
                        .strong()
                        .color(theme::TEXT)
                        .monospace(),
                );
            });
        });
        let h = 6.0;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(3),
            Color32::from_white_alpha(14),
        );
        let v = value.clamp(0.0, 1.0);
        if v > 0.005 {
            let fw = rect.width() * v;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(fw, h)),
                egui::CornerRadius::same(3),
                color,
            );
            // 填充末端的小圆点，增加精致感
            ui.painter().circle_filled(
                egui::pos2(rect.left() + fw, rect.center().y),
                2.2,
                color,
            );
        }
    });
}

/// 统计卡片：固定宽度，居中大数字 + 标签（四宫格等宽）。
fn stat_card(ui: &mut egui::Ui, width: f32, label: &str, value: u64, color: Color32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 62.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            egui::Frame::new()
                .fill(theme::CARD)
                .stroke(egui::Stroke::new(1.0, theme::LINE))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(4, 9))
                .show(ui, |ui| {
                    ui.set_min_width(width - 10.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(value.to_string())
                                .size(22.0)
                                .strong()
                                .color(color)
                                .monospace(),
                        );
                        ui.label(egui::RichText::new(label).size(11.5).color(theme::TEXT_DIM));
                    });
                });
        },
    );
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ---- 快照共享状态（克隆后释放锁） ----
        let (running, paused, state, stats, last_sim, audio_level, game_focused, foreground_exe,
            devices, logs) = {
            let s = self.shared.lock().unwrap();
            (
                s.running,
                s.paused,
                s.state,
                s.stats,
                s.last_sim,
                s.audio_level,
                s.game_focused,
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
                let (pin_click, gear_click) = title_bar(ui, pinned, settings_open);
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

                                        meter(ui, "咬钩相似度", sim_display, theme::CYAN);
                                        ui.add_space(14.0);
                                        meter(ui, "输出音量", audio_level, theme::BLUE);
                                        ui.add_space(14.0);

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
                                                    "阈值 {:.0}%",
                                                    cfg.bite_threshold
                                                ))
                                                .size(11.0)
                                                .color(theme::TEXT_DIM),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new("内置咬钩音模板")
                                                            .size(11.0)
                                                            .color(theme::TEXT_DIM),
                                                    );
                                                },
                                            );
                                        });
                                    });

                                ui.add_space(12.0);

                                // ---- 统计（四宫格等宽） ----
                                ui.horizontal(|ui| {
                                    let sp = ui.spacing().item_spacing.x;
                                    let w = (ui.available_width() - sp * 3.0) / 4.0;
                                    stat_card(ui, w, "抛竿", stats.casts, theme::CYAN);
                                    stat_card(ui, w, "咬钩", stats.bites, theme::AMBER);
                                    stat_card(ui, w, "钓获", stats.catches, theme::MINT);
                                    stat_card(ui, w, "脱钩", stats.misses, theme::RED);
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
            });

        // ---- 设置弹窗 ----
        if settings_toggled {
            self.show_settings = !self.show_settings;
        }
        let mut settings_open = self.show_settings;
        let mut close_settings = false;
        egui::Window::new(egui::RichText::new("设置").strong().color(theme::TEXT))
            .open(&mut settings_open)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .min_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(ui.style())
                    .fill(Color32::from_rgb(24, 40, 31))
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("⚙  设置").strong().size(15.0).color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() { close_settings = true; }
                    });
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(8.0);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                ui.add_enabled_ui(!running, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("游戏进程");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut cfg.game_exe).desired_width(220.0),
                            )
                            .changed()
                        {
                            cfg_changed = true;
                        }
                    });

                    let mut sel: usize = if cfg.device_name.is_empty() {
                        0
                    } else {
                        devices
                            .iter()
                            .position(|d| *d == cfg.device_name)
                            .map(|i| i + 1)
                            .unwrap_or(0)
                    };
                    let sel_text = if sel == 0 {
                        "默认输出".to_string()
                    } else {
                        devices.get(sel - 1).cloned().unwrap_or_default()
                    };
                    egui::ComboBox::from_id_salt("audio-device")
                        .width(ui.available_width())
                        .selected_text(sel_text)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(&mut sel, 0usize, "默认输出")
                                .changed()
                            {
                                cfg.device_name = String::new();
                                cfg_changed = true;
                            }
                            for (i, d) in devices.iter().enumerate() {
                                if ui
                                    .selectable_value(&mut sel, i + 1, d.as_str())
                                    .changed()
                                {
                                    cfg.device_name = d.clone();
                                    cfg_changed = true;
                                }
                            }
                        });

                    ui.add_space(4.0);
                    if ui
                        .add(
                            egui::Slider::new(&mut cfg.bite_threshold, 0.0..=100.0)
                                .text("咬钩阈值")
                                .suffix(" %"),
                        )
                        .changed()
                    {
                        cfg_changed = true;
                    }

                    ui.add_space(4.0);
                    egui::Grid::new("cfg-grid")
                        .num_columns(4)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("抛竿等待");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cfg.cast_delay)
                                        .speed(0.1)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                cfg_changed = true;
                            }
                            ui.label("刺鱼延迟");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cfg.strike_delay)
                                        .speed(0.05)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                cfg_changed = true;
                            }
                            ui.end_row();

                            ui.label("收竿等待");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cfg.reel_wait_min)
                                        .speed(0.1)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                cfg_changed = true;
                            }
                            ui.label("— 上限");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cfg.reel_wait_max)
                                        .speed(0.1)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                cfg_changed = true;
                            }
                            ui.end_row();

                            ui.label("单轮超时");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cfg.round_timeout)
                                        .speed(0.5)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                cfg_changed = true;
                            }
                            ui.label("点击时长");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut cfg.click_hold)
                                        .speed(0.01)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                cfg_changed = true;
                            }
                            ui.end_row();
                        });

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut cfg.double_cast, "抛竿双击").changed() {
                            cfg_changed = true;
                        }
                        if ui.checkbox(&mut cfg.hold_rmb, "等待时按住右键（仅前台模式）").changed() {
                            cfg_changed = true;
                        }
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









