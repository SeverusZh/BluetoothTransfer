//! 原生文件对话框：阻塞调用，调用方负责放到后台线程，避免卡住界面。

use std::path::PathBuf;

use bt_core::BtError;

/// 文件选择结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickedFiles {
    pub paths: Vec<PathBuf>,
    pub cancelled: bool,
}

/// 多选文件。
pub fn pick_files(title: &str) -> PickedFiles {
    let selection = rfd::FileDialog::new().set_title(title).pick_files();
    match selection {
        Some(paths) if !paths.is_empty() => PickedFiles {
            paths,
            cancelled: false,
        },
        _ => PickedFiles {
            paths: Vec::new(),
            cancelled: true,
        },
    }
}

/// 选择单个文件夹。
pub fn pick_folder(title: &str, start: Option<&std::path::Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = start.filter(|dir| dir.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// 选择保存路径；用户取消时返回 None。
pub fn save_file(title: &str, default_name: &str, extension: &str) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(default_name)
        .add_filter(extension.to_ascii_uppercase(), &[extension]);
    dialog.save_file()
}

/// 在后台线程运行对话框，结果经通道回传。
pub fn spawn<T, F>(work: F) -> crossbeam_channel::Receiver<Result<T, BtError>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, BtError> + Send + 'static,
{
    let (sender, receiver) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("bt-dialog".to_string())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .ok();
    receiver
}
