//! 深色玻璃拟态主题（配色取自 DeltaMelody 的 Windows 界面）。
//!
//! 映射关系：mint #bcf79b / cyan #9eecdc / amber #f3c77b / blue #99d6ff，
//! 玻璃背景为半透明深绿，配合 Windows 亚克力(DWM) 透出系统模糊。

use egui::{Color32, FontFamily, FontId, Stroke, TextStyle};

pub const MINT: Color32 = Color32::from_rgb(0xbc, 0xf7, 0x9b);
pub const CYAN: Color32 = Color32::from_rgb(0x9e, 0xec, 0xdc);
pub const AMBER: Color32 = Color32::from_rgb(0xf3, 0xc7, 0x7b);
pub const BLUE: Color32 = Color32::from_rgb(0x99, 0xd6, 0xff);
pub const RED: Color32 = Color32::from_rgb(0xfb, 0x86, 0x86);
/// 玻璃面板底色（半透明，让背后的亚克力模糊透出）。
pub const GLASS: Color32 = Color32::from_rgba_unmultiplied_const(12, 22, 17, 208);
pub const PANEL: Color32 = Color32::from_rgba_unmultiplied_const(22, 38, 30, 150);
pub const TEXT: Color32 = Color32::from_rgb(0xee, 0xf3, 0xea);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xa5, 0xbe, 0x9e);
/// 细边框（亮色，仿 DeltaMelody 的 rgba(220,239,223,0.15)）。
pub const LINE: Color32 = Color32::from_rgba_unmultiplied_const(220, 239, 223, 38);
/// 卡片底色（半透明白，叠加在玻璃上形成层次）。
pub const CARD: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 10);
/// 标题栏底色（半透明深色条）。
pub const TITLEBAR: Color32 = Color32::from_rgba_unmultiplied_const(14, 24, 19, 200);

/// 加载中文字体并应用深色主题。
pub fn apply(ctx: &egui::Context) {
    setup_fonts(ctx);

    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = GLASS;
    v.window_fill = GLASS;
    v.extreme_bg_color = PANEL;
    v.faint_bg_color = Color32::from_rgb(22, 34, 29);
    v.code_bg_color = Color32::from_rgb(18, 26, 23);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.selection.bg_fill = Color32::from_rgb(0x44, 0x6b, 0x52);
    v.selection.stroke = Stroke::NONE;
    v.hyperlink_color = CYAN;

    let inactive = Color32::from_rgb(30, 46, 38);
    let hovered = Color32::from_rgb(38, 58, 48);
    let active = Color32::from_rgb(46, 68, 56);
    v.widgets.inactive.bg_fill = inactive;
    v.widgets.inactive.weak_bg_fill = inactive;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    v.widgets.hovered.bg_fill = hovered;
    v.widgets.hovered.weak_bg_fill = hovered;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, MINT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.bg_fill = active;
    v.widgets.active.weak_bg_fill = active;
    v.widgets.active.bg_stroke = Stroke::new(1.0, MINT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(v);

    ctx.global_style_mut(|style| {
        style.text_styles.insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(12.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(14.0, FontFamily::Proportional));
    });
}

/// 从 Windows 字体目录加载一款 CJK 字体，保证中文正常显示。
fn setup_fonts(ctx: &egui::Context) {
    use std::sync::Arc;

    let mut fonts = egui::FontDefinitions::default();
    // 优先单个 TTF（DengXian），再回退到微软雅黑 / 黑体 / 宋体。
    let candidates = ["Deng.ttf", "msyh.ttc", "simhei.ttf", "simsun.ttc"];
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
///
/// - Win11：`DWMSBT_TRANSIENTWINDOW`（亚克力，自动模糊窗口背后内容）。
/// - Win10：调用会静默失败，此时仍由半透明玻璃底色兜底。
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
        // DWMWCP_ROUND = 2
        let corner: i32 = 2;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// 非 Windows 平台的空实现（保持接口一致）。
#[cfg(not(windows))]
pub fn apply_backdrop(_hwnd: isize) {}