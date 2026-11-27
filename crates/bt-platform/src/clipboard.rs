//! 剪贴板：文本、图片与文件列表；Linux 上对象需要常驻以维持选区所有权。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use bt_core::BtError;

/// 图片剪贴板内容，RGBA8 逐行自上而下。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// 剪贴板内容：文件列表、纯文本或图片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Paths(Vec<PathBuf>),
    Text(String),
    Image(ClipboardImage),
}

/// 剪贴板读写。
pub trait Clipboard: Send + Sync {
    fn set(&self, content: ClipboardContent) -> Result<(), BtError>;

    fn get_text(&self) -> Result<Option<String>, BtError>;

    fn get_image(&self) -> Result<Option<ClipboardImage>, BtError> {
        Err(crate::unsupported("读取剪贴板图片"))
    }
}

/// 系统剪贴板。arboard 的 Clipboard 必须常驻，否则 Linux 上的选区会立即失效。
#[derive(Debug, Default)]
pub struct SystemClipboard;

fn shared() -> Result<&'static Mutex<arboard::Clipboard>, BtError> {
    static CLIPBOARD: OnceLock<Result<Mutex<arboard::Clipboard>, String>> = OnceLock::new();
    let entry = CLIPBOARD.get_or_init(|| {
        arboard::Clipboard::new()
            .map(Mutex::new)
            .map_err(|error| format!("无法访问系统剪贴板：{error}"))
    });
    match entry {
        Ok(clipboard) => Ok(clipboard),
        Err(error) => Err(BtError::new(bt_core::BtErrorKind::Unsupported, error.clone())),
    }
}

impl Clipboard for SystemClipboard {
    fn set(&self, content: ClipboardContent) -> Result<(), BtError> {
        let clipboard = shared()?;
        let mut clipboard = clipboard.lock().map_err(|_| BtError::internal("剪贴板锁被投毒"))?;
        match content {
            ClipboardContent::Text(text) => clipboard
                .set()
                .text(text)
                .map_err(|error| clipboard_error("写入文本", error)),
            ClipboardContent::Paths(paths) => {
                if paths.is_empty() {
                    return Err(BtError::invalid("待复制文件列表为空"));
                }
                clipboard
                    .set()
                    .file_list(&paths)
                    .map_err(|error| clipboard_error("写入文件列表", error))
            }
            ClipboardContent::Image(image) => {
                if image.width == 0 || image.height == 0 || image.rgba.len() != image.width * image.height * 4 {
                    return Err(BtError::invalid("图片尺寸与像素数据不一致"));
                }
                let data = arboard::ImageData {
                    width: image.width,
                    height: image.height,
                    bytes: std::borrow::Cow::Owned(image.rgba),
                };
                clipboard
                    .set()
                    .image(data)
                    .map_err(|error| clipboard_error("写入图片", error))
            }
        }
    }

    fn get_text(&self) -> Result<Option<String>, BtError> {
        let clipboard = shared()?;
        let mut clipboard = clipboard.lock().map_err(|_| BtError::internal("剪贴板锁被投毒"))?;
        match clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(clipboard_error("读取文本", error)),
        }
    }

    fn get_image(&self) -> Result<Option<ClipboardImage>, BtError> {
        let clipboard = shared()?;
        let mut clipboard = clipboard.lock().map_err(|_| BtError::internal("剪贴板锁被投毒"))?;
        match clipboard.get_image() {
            Ok(image) => Ok(Some(ClipboardImage {
                width: image.width,
                height: image.height,
                rgba: image.bytes.into_owned(),
            })),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(clipboard_error("读取图片", error)),
        }
    }
}

fn clipboard_error(action: &str, error: arboard::Error) -> BtError {
    BtError::new(bt_core::BtErrorKind::Io, format!("剪贴板{action}失败：{error}"))
}

/// 按当前平台返回剪贴板实现。
pub fn platform_clipboard() -> Box<dyn Clipboard> {
    Box::new(SystemClipboard)
}

/// 在界面线程预热剪贴板，避免首次操作发生在后台线程导致 Linux 选区归属异常。
pub fn warmup() -> Result<(), BtError> {
    let clipboard = shared()?;
    let _guard = clipboard.lock().map_err(|_| BtError::internal("剪贴板锁被投毒"))?;
    Ok(())
}

/// 把 PNG/JPEG 等常见图片读成 RGBA；超过 64MB 不解码，避免占用大量内存。
pub fn read_image_file(path: &Path) -> Result<ClipboardImage, BtError> {
    const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(BtError::invalid(format!(
            "图片文件 {} 为 {} 字节，超过 64MB 解码上限",
            path.display(),
            metadata.len()
        )));
    }
    let image = image::open(path).map_err(|error| BtError::invalid(format!("图片解码失败：{error}")))?;
    let rgba = image.to_rgba8();
    Ok(ClipboardImage {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    })
}

/// 把 RGBA 图片写成 PNG；用于把剪贴板图片转成可发送的临时文件。
pub fn save_png(path: &Path, image: &ClipboardImage) -> Result<(), BtError> {
    if image.width == 0 || image.height == 0 || image.rgba.len() != image.width * image.height * 4 {
        return Err(BtError::invalid("图片尺寸与像素数据不一致"));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let buffer = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.rgba.clone())
        .ok_or_else(|| BtError::invalid("图片尺寸与像素数据不一致"))?;
    buffer
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|error| BtError::new(bt_core::BtErrorKind::Io, format!("写入图片失败：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_preserves_pixels() {
        let dir = std::env::temp_dir().join(format!("bt-platform-clipboard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.png");
        let image = ClipboardImage {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        save_png(&path, &image).unwrap();
        assert_eq!(read_image_file(&path).unwrap(), image);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_inconsistent_image_buffers() {
        let dir = std::env::temp_dir().join(format!("bt-platform-clipboard-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = ClipboardImage {
            width: 3,
            height: 3,
            rgba: vec![0; 4],
        };
        assert!(save_png(&dir.join("bad.png"), &image).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
