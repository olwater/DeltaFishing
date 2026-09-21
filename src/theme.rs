//! 深色玻璃拟态与战术科技主题。
//! 配色方案：现代黑曜石深色背景，搭配战术薄荷绿、青空蓝、琥珀黄与珊瑚红。

use egui::{Color32, FontFamily, FontId, Stroke, TextStyle};

// ---- 核心强调色 ----
pub const MINT: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);        // 活力薄荷绿 #4ade80
#[allow(dead_code)]
pub const MINT_GLOW: Color32 = Color32::from_rgba_unmultiplied_const(74, 222, 128, 45);
pub const CYAN: Color32 = Color32::from_rgb(0x38, 0xbd, 0xf8);        // 青空蓝 #38bdf8
pub const AMBER: Color32 = Color32::from_rgb(0xfb, 0xbf, 0x24);       // 琥珀黄 #fbbf24
pub const BLUE: Color32 = Color32::from_rgb(0x81, 0x8c, 0xf8);        // 靛蓝 #818cf8
pub const RED: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);         // 珊瑚红 #f87171

// ---- 背景与表面层次 ----
pub const GLASS: Color32 = Color32::from_rgba_unmultiplied_const(12, 18, 15, 230); // 亚克力主背景
pub const TITLEBAR: Color32 = Color32::from_rgba_unmultiplied_const(16, 24, 20, 240);
pub const CARD: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 8);   // 卡片轻微白透明
#[allow(dead_code)]
pub const CARD_ELEVATED: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 14);
pub const PANEL: Color32 = Color32::from_rgba_unmultiplied_const(20, 32, 26, 180);

// ---- 文本色阶 ----
pub const TEXT: Color32 = Color32::from_rgb(0xf3, 0xf4, 0xf6);        // 主文本 #f3f4f6
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9c, 0xa3, 0xaf);    // 次级文本 #9ca3af
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x64, 0x74, 0x8b);  // 弱化文本 #64748b

// ---- 边框与线段 ----
pub const LINE: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 22);
pub const LINE_LIGHT: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 45);

/// 加载中文字体并应用深色主题。
pub fn apply(ctx: &egui::Context) {
    setup_fonts(ctx);

    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = GLASS;
    v.window_fill = GLASS;
    v.extreme_bg_color = PANEL;
    v.faint_bg_color = Color32::from_rgb(18, 28, 23);
    v.code_bg_color = Color32::from_rgb(15, 22, 19);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied_const(74, 222, 128, 60);
    v.selection.stroke = Stroke::NONE;
    v.hyperlink_color = CYAN;

    let inactive = Color32::from_rgba_unmultiplied_const(255, 255, 255, 10);
    let hovered = Color32::from_rgba_unmultiplied_const(255, 255, 255, 20);
    let active = Color32::from_rgba_unmultiplied_const(74, 222, 128, 40);

    v.widgets.inactive.bg_fill = inactive;
    v.widgets.inactive.weak_bg_fill = inactive;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);

    v.widgets.hovered.bg_fill = hovered;
    v.widgets.hovered.weak_bg_fill = hovered;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, MINT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);

    v.widgets.active.bg_fill = active;
    v.widgets.active.weak_bg_fill = active;
    v.widgets.active.bg_stroke = Stroke::new(1.0, MINT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.corner_radius = egui::CornerRadius::same(6);

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(v);

    ctx.global_style_mut(|style| {
        style.text_styles.insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(13.5, FontFamily::Proportional));
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}

/// 从 Windows 字体目录加载一款 CJK 字体，保证中文正常显示。
fn setup_fonts(ctx: &egui::Context) {
    use std::sync::Arc;

    let mut fonts = egui::FontDefinitions::default();
    let candidates = ["msyh.ttc", "Deng.ttf", "simhei.ttf", "simsun.ttc"];
    let dir = std::path::Path::new("C:\\Windows\\Fonts");
    for name in candidates {
        let path = dir.join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            fonts
                .font_data
                .insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
            if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
                list.insert(0, name.to_string());
            }
            if let Some(list) = fonts.families.get_mut(&FontFamily::Monospace) {
                list.insert(0, name.to_string());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

/// 对无边框窗口应用 Windows 亚克力背景与 DWM 圆角。
#[cfg(windows)]
pub fn apply_backdrop(hwnd: isize) {
    use windows_sys::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
            DWMWA_WINDOW_CORNER_PREFERENCE,
        },
    };

    let hwnd = hwnd as HWND;
    unsafe {
        let backdrop: i32 = DWMSBT_TRANSIENTWINDOW;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        let corner: i32 = 2; // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
pub fn apply_backdrop(_hwnd: isize) {}
