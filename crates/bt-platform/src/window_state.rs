use std::path::Path;

/// 主窗口几何状态，跨会话恢复。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 960.0,
            height: 640.0,
            x: 80.0,
            y: 80.0,
            maximized: false,
        }
    }
}

impl WindowState {
    /// 读取窗口状态；文件缺失或损坏时返回默认值。
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut state = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "width" => state.width = value.trim().parse().unwrap_or(state.width),
                "height" => state.height = value.trim().parse().unwrap_or(state.height),
                "x" => state.x = value.trim().parse().unwrap_or(state.x),
                "y" => state.y = value.trim().parse().unwrap_or(state.y),
                "maximized" => state.maximized = value.trim() == "true",
                _ => {}
            }
        }
        state.clamp()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let state = self.clamp();
        let text = format!(
            "width={}\nheight={}\nx={}\ny={}\nmaximized={}\n",
            state.width, state.height, state.x, state.y, state.maximized
        );
        std::fs::write(path, text)
    }

    fn clamp(mut self) -> Self {
        self.width = self.width.clamp(480.0, 7680.0);
        self.height = self.height.clamp(360.0, 4320.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_default_when_missing() {
        assert_eq!(
            WindowState::load(Path::new("/nonexistent/window.state")),
            WindowState::default()
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join("bt-platform-window");
        let path = dir.join("window.state");
        let state = WindowState {
            width: 1280.0,
            height: 720.0,
            x: 12.0,
            y: 24.0,
            maximized: true,
        };
        state.save(&path).unwrap();
        assert_eq!(WindowState::load(&path), state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
