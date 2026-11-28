//! 窗口图标：内嵌品牌 PNG，启动时解码为 RGBA，不依赖外部文件或显示服务。

use eframe::egui::IconData;

/// 与安装器共用的品牌图标源文件。
pub const APP_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/windows/app.png"));

/// 解码窗口图标；资源损坏时返回 None 由调用方退回系统默认图标。
pub fn window_icon() -> Option<IconData> {
    let decoded = image::load_from_memory_with_format(APP_PNG, image::ImageFormat::Png).ok()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    Some(IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_png_decodes_to_square_rgba() {
        let icon = window_icon().expect("内嵌品牌图标可解码");
        assert_eq!(icon.width, icon.height);
        assert!(icon.width >= 16);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
