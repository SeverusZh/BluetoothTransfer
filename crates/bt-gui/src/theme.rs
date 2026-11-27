//! 视觉样式与 CJK 字体发现：不内嵌字体，运行时从系统字体目录挑选。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, TextStyle};

pub const BRAND: Color32 = Color32::from_rgb(15, 108, 189);
pub const BRAND_DARK: Color32 = Color32::from_rgb(10, 82, 148);
pub const DANGER: Color32 = Color32::from_rgb(196, 43, 28);
pub const SUCCESS: Color32 = Color32::from_rgb(16, 124, 16);
pub const WARNING: Color32 = Color32::from_rgb(154, 103, 0);
pub const TEXT: Color32 = Color32::from_rgb(26, 31, 42);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(87, 97, 111);
pub const BACKGROUND: Color32 = Color32::from_rgb(244, 246, 249);
pub const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
pub const BORDER: Color32 = Color32::from_rgb(226, 231, 239);

/// 字体装载结果，用于自检与状态提示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontReport {
    pub path: Option<PathBuf>,
    pub detail: String,
}

impl FontReport {
    pub fn found(&self) -> bool {
        self.path.is_some()
    }
}

/// 设置浅色主题：品牌蓝作强调色，卡片与输入框带细边框。
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = SURFACE;
    visuals.faint_bg_color = Color32::from_rgb(247, 249, 252);
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = BRAND;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = DANGER;
    visuals.selection.bg_fill = Color32::from_rgb(213, 231, 250);
    visuals.selection.stroke = Stroke::new(1.0, BRAND_DARK);
    visuals.widgets.noninteractive.bg_fill = BACKGROUND;
    visuals.widgets.noninteractive.weak_bg_fill = BACKGROUND;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(237, 241, 246);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(237, 241, 246);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(201, 210, 222));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.active.bg_fill = Color32::from_rgb(227, 233, 241);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(227, 233, 241);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, BRAND);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.bg_fill = SURFACE;
    visuals.widgets.open.weak_bg_fill = SURFACE;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, BRAND);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.spacing.interact_size = egui::vec2(0.0, 28.0);
        style.spacing.combo_width = 150.0;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
        style.visuals.widgets.active.corner_radius = CornerRadius::same(6);
        style
            .text_styles
            .insert(TextStyle::Heading, egui::FontId::proportional(20.0));
        style
            .text_styles
            .insert(TextStyle::Body, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Button, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Small, egui::FontId::proportional(11.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, egui::FontId::monospace(12.0));
    });
}

/// 从系统字体目录挑选并装载 CJK 字体；找不到时保持默认字体。
pub fn install_cjk_font(ctx: &egui::Context) -> FontReport {
    let Some((path, index)) = discover_font() else {
        return FontReport {
            path: None,
            detail: "未找到可用的中文系统字体，中文可能显示为方框；可设置 BT_FONT_PATH 指向字体文件".to_string(),
        };
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return FontReport {
                path: None,
                detail: format!("读取字体 {} 失败：{error}", path.display()),
            }
        }
    };
    let mut definitions = FontDefinitions::default();
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    definitions.font_data.insert("cjk".to_string(), Arc::new(data));
    if let Some(family) = definitions.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "cjk".to_string());
    }
    if let Some(family) = definitions.families.get_mut(&FontFamily::Monospace) {
        family.push("cjk".to_string());
    }
    ctx.set_fonts(definitions);
    FontReport {
        path: Some(path.clone()),
        detail: format!("已装载 {}", path.display()),
    }
}

/// 候选字体：环境变量优先，其次用户字体目录，最后系统字体目录。
pub fn discover_font() -> Option<(PathBuf, u32)> {
    if let Some(value) = std::env::var_os("BT_FONT_PATH").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        let index = std::env::var("BT_FONT_INDEX")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if valid_font(&path, index) {
            return Some((path, index));
        }
    }
    for candidate in candidates() {
        for index in 0..4 {
            if valid_font(&candidate, index) {
                return Some((candidate, index));
            }
        }
    }
    None
}

fn candidates() -> Vec<PathBuf> {
    let mut list = Vec::new();
    if let Some(home) = bt_platform::paths::home_dir() {
        for user_fonts in [home.join(".local").join("share").join("fonts"), home.join(".fonts")] {
            for name in ["msyh.ttc", "msyh.ttf", "msyhbd.ttc", "simhei.ttf", "simsun.ttc"] {
                list.push(user_fonts.join(name));
            }
        }
    }
    if cfg!(windows) {
        let root = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let fonts = root.join("Fonts");
        for name in ["msyh.ttc", "msyhbd.ttc", "simhei.ttf", "simsun.ttc"] {
            list.push(fonts.join(name));
        }
    } else {
        for path in [
            "/usr/share/fonts/truetype/microsoft/msyh.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
            "/usr/share/fonts/truetype/arphic/uming.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ] {
            list.push(PathBuf::from(path));
        }
    }
    list
}

fn valid_font(path: &Path, index: u32) -> bool {
    if index > 3 {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    if bytes.len() < 16 {
        return false;
    }
    match &bytes[..4] {
        b"ttcf" => {
            let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
            (1..=16).contains(&count) && index < count
        }
        [0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" => index == 0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_fonts_are_rejected_without_panic() {
        assert!(!valid_font(Path::new("/nonexistent/blue-tooth-font.ttf"), 0));
        let dir = std::env::temp_dir().join(format!("bt-gui-font-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.ttf");
        std::fs::write(&path, b"not a font").unwrap();
        assert!(!valid_font(&path, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
