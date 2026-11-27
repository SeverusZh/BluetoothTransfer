use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use flate2::write::DeflateEncoder;
use flate2::Compression;

use bt_core::{paths, BtError};

pub const PACKAGE_EXTENSION: &str = "zip";
pub const PENDING_PREFIX: &str = "bt_asst_pending_";

/// 打包计划：文件夹整体压缩成一个 zip 再走任一条通道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub sources: Vec<PathBuf>,
    pub archive_name: String,
    pub estimated_bytes: u64,
}

/// 已经生成并可直接入队的载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedSource {
    pub path: PathBuf,
    pub display_name: String,
    pub owned: bool,
}

/// 生成打包计划，不触碰文件内容。
pub fn plan_folder_package(folder: &Path) -> Result<PackagePlan, BtError> {
    if !folder.is_dir() {
        return Err(BtError::not_found(format!("目录不存在 {}", folder.display())));
    }
    let raw_name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string());
    let archive_name = format!("{}.{PACKAGE_EXTENSION}", paths::sanitize_file_name(&raw_name));
    Ok(PackagePlan {
        sources: vec![folder.to_path_buf()],
        archive_name,
        estimated_bytes: 0,
    })
}

/// 把文件夹压缩为 zip 写入目标路径。
pub fn package_folder(folder: &Path, target: &Path) -> Result<PathBuf, BtError> {
    let plan = plan_folder_package(folder)?;
    write_zip(target, &plan.sources, ZipMode::Folder)?;
    Ok(target.to_path_buf())
}

/// 把单个文件压缩为单条目 zip。
pub fn package_file(source: &Path, target: &Path) -> Result<PathBuf, BtError> {
    if !source.is_file() {
        return Err(BtError::not_found(format!("文件不存在 {}", source.display())));
    }
    write_zip(target, &[source.to_path_buf()], ZipMode::File)?;
    Ok(target.to_path_buf())
}

/// 待续传打包产物目录管理：zip 与文本临时文件在传输完成前保留，保证 SHA-256 可复现。
#[derive(Debug, Clone)]
pub struct PendingPackageStore {
    directory: PathBuf,
}

impl PendingPackageStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn ensure(&self) -> Result<(), BtError> {
        std::fs::create_dir_all(&self.directory)?;
        Ok(())
    }

    fn new_path(&self, extension: &str) -> Result<PathBuf, BtError> {
        self.ensure()?;
        Ok(self.directory.join(format!(
            "{PENDING_PREFIX}{}.{extension}",
            bt_core::hex_lower(&random_bytes())
        )))
    }

    pub fn create_zip_from_file(&self, source: &Path) -> Result<PackedSource, BtError> {
        let path = self.new_path(PACKAGE_EXTENSION)?;
        package_file(source, &path)?;
        let display_name = format!(
            "{}.zip",
            source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string())
        );
        Ok(PackedSource {
            path,
            display_name,
            owned: true,
        })
    }

    pub fn create_zip_from_folder(&self, folder: &Path) -> Result<PackedSource, BtError> {
        let plan = plan_folder_package(folder)?;
        let path = self.new_path(PACKAGE_EXTENSION)?;
        package_folder(folder, &path)?;
        Ok(PackedSource {
            path,
            display_name: plan.archive_name,
            owned: true,
        })
    }

    pub fn create_text_file(&self, text: &str, file_name: &str) -> Result<PackedSource, BtError> {
        let cleaned = paths::sanitize_file_name(file_name);
        let extension = Path::new(&cleaned)
            .extension()
            .map(|value| value.to_string_lossy().into_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "txt".to_string());
        let path = self.new_path(&extension)?;
        std::fs::write(&path, text.as_bytes())?;
        Ok(PackedSource {
            path,
            display_name: cleaned,
            owned: true,
        })
    }

    /// 路径是否位于本产物目录内，删除只允许命中自己生成的产物。
    pub fn is_owned(&self, path: &Path) -> bool {
        let Ok(root) = self
            .directory
            .canonicalize()
            .or_else(|_| absolute_lexical(&self.directory))
        else {
            return false;
        };
        let candidate = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        candidate.starts_with(&root) && candidate != root
    }

    pub fn delete(&self, path: &Path) -> bool {
        if !self.is_owned(path) {
            return false;
        }
        match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(_) => false,
        }
    }

    /// 清理未被引用且超过保留期的孤儿产物。
    pub fn clean_orphans(&self, referenced: &[PathBuf], older_than: Duration) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return 0;
        };
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if referenced.iter().any(|item| item == &path) {
                continue;
            }
            let stale = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .is_some_and(|modified| modified < cutoff);
            if stale && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        Ok(paths::normalize_lexically(path))
    } else {
        Ok(paths::normalize_lexically(&std::env::current_dir()?.join(path)))
    }
}

fn random_bytes() -> [u8; 16] {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let mut seed = Vec::with_capacity(32);
    seed.extend_from_slice(&now.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&counter.to_le_bytes());
    let digest = bt_core::sha256(&seed);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

enum ZipMode {
    File,
    Folder,
}

const ZIP32_MAX: u64 = u32::MAX as u64;

struct ZipItem {
    source: PathBuf,
    name: String,
}

fn collect_zip_items(sources: &[PathBuf], mode: &ZipMode) -> Result<Vec<ZipItem>, BtError> {
    let mut items = Vec::new();
    match mode {
        ZipMode::File => {
            for source in sources {
                let name = source
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".to_string());
                items.push(ZipItem {
                    source: source.clone(),
                    name: paths::sanitize_file_name(&name),
                });
            }
        }
        ZipMode::Folder => {
            for source in sources {
                let mut files = Vec::new();
                collect_files(source, source, &mut files)?;
                files.sort();
                for (path, relative) in files {
                    items.push(ZipItem {
                        source: path,
                        name: relative.to_string_lossy().replace('\\', "/"),
                    });
                }
            }
        }
    }
    Ok(items)
}

/// ZIP32 字段上限检查：单文件与累计未压缩大小必须小于 4 GiB。
fn check_zip32_limits(items: &[ZipItem]) -> Result<(), BtError> {
    if items.len() > u16::MAX as usize {
        return Err(BtError::unsupported(format!(
            "zip 条目数 {} 超过 ZIP32 上限；请拆分文件夹",
            items.len()
        )));
    }
    let mut total = 0u64;
    for item in items {
        if item.name.len() > u16::MAX as usize {
            return Err(BtError::invalid("zip 条目名过长"));
        }
        let metadata = std::fs::metadata(&item.source).map_err(|error| {
            let message = format!("文件不可读：{}（{error}）", item.source.display());
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => BtError::permission(message),
                _ => BtError::not_found(message),
            }
        })?;
        let size = metadata.len();
        if size > ZIP32_MAX {
            return Err(BtError::unsupported(format!(
                "{} 大小 {size} 字节，超过单文件 zip 的 4 GiB 上限；请直接发送或先拆分文件",
                item.source.display()
            )));
        }
        total = total.saturating_add(size);
        if total > ZIP32_MAX {
            return Err(BtError::unsupported(format!(
                "待打包内容累计 {total} 字节，超过 ZIP32 的 4 GiB 上限；请拆分文件夹或改用不打包发送"
            )));
        }
    }
    Ok(())
}

/// 校验文件夹打包是否满足 ZIP32 上限，不写产物；供上层在入队前给出命令级错误。
pub fn check_folder_zip32(folder: &Path) -> Result<(), BtError> {
    let plan = plan_folder_package(folder)?;
    let items = collect_zip_items(&plan.sources, &ZipMode::Folder)?;
    check_zip32_limits(&items)
}

fn write_zip(target: &Path, sources: &[PathBuf], mode: ZipMode) -> Result<(), BtError> {
    let items = collect_zip_items(sources, &mode)?;
    check_zip32_limits(&items)?;
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let result = write_zip_items(target, &items);
    if result.is_err() {
        let _ = std::fs::remove_file(target);
    }
    result
}

fn write_zip_items(target: &Path, items: &[ZipItem]) -> Result<(), BtError> {
    let mut writer = ZipWriter {
        file: File::create(target)?,
        entries: Vec::new(),
    };
    for item in items {
        writer.add_file(&item.source, &item.name)?;
    }
    writer.finish()
}

struct EntryRecord {
    name: Vec<u8>,
    crc: u32,
    compressed: u32,
    uncompressed: u32,
    offset: u32,
}

struct ZipWriter {
    file: File,
    entries: Vec<EntryRecord>,
}

impl ZipWriter {
    fn add_file(&mut self, source: &Path, name: &str) -> Result<(), BtError> {
        let header_offset = self.file.stream_position()?;
        if header_offset > ZIP32_MAX {
            return Err(BtError::unsupported("zip 数据偏移超出 ZIP32 上限，已停止打包"));
        }
        let mut input = File::open(source)?;
        let size = input.metadata()?.len();
        if size > ZIP32_MAX {
            return Err(BtError::unsupported(format!(
                "{} 大小 {size} 字节，超过单文件 zip 的 4 GiB 上限；请直接发送或先拆分文件",
                source.display()
            )));
        }
        let crc = streaming_crc32(&mut input)?;
        input.seek(SeekFrom::Start(0))?;
        let uncompressed = size as u32;
        let name_bytes = name.as_bytes().to_vec();
        if name_bytes.len() > u16::MAX as usize {
            return Err(BtError::invalid("zip 条目名过长"));
        }
        self.file.write_all(&0x0403_4b50u32.to_le_bytes())?;
        self.file.write_all(&20u16.to_le_bytes())?;
        self.file.write_all(&0x0800u16.to_le_bytes())?;
        self.file.write_all(&8u16.to_le_bytes())?;
        self.file.write_all(&0x0021u16.to_le_bytes())?;
        self.file.write_all(&0x0000u16.to_le_bytes())?;
        self.file.write_all(&0u32.to_le_bytes())?;
        self.file.write_all(&0u32.to_le_bytes())?;
        self.file.write_all(&0u32.to_le_bytes())?;
        self.file.write_all(&(name_bytes.len() as u16).to_le_bytes())?;
        self.file.write_all(&0u16.to_le_bytes())?;
        self.file.write_all(&name_bytes)?;

        let mut encoder = DeflateEncoder::new(&mut self.file, Compression::default());
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            encoder.write_all(&buffer[..read])?;
        }
        encoder.finish()?;
        let data_end = self.file.stream_position()?;
        if data_end > ZIP32_MAX {
            return Err(BtError::unsupported("zip 数据总量超出 ZIP32 上限，已停止打包"));
        }
        let compressed_size = data_end - header_offset - 30 - name_bytes.len() as u64;
        if compressed_size > ZIP32_MAX {
            return Err(BtError::unsupported("压缩后条目大小超出 ZIP32 上限，已停止打包"));
        }
        let compressed = compressed_size as u32;
        self.file.seek(SeekFrom::Start(header_offset + 14))?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&compressed.to_le_bytes())?;
        self.file.write_all(&uncompressed.to_le_bytes())?;
        self.file.seek(SeekFrom::Start(data_end))?;
        self.entries.push(EntryRecord {
            name: name_bytes,
            crc,
            compressed,
            uncompressed,
            offset: header_offset as u32,
        });
        Ok(())
    }

    fn finish(mut self) -> Result<(), BtError> {
        let start = self.file.stream_position()?;
        if start > ZIP32_MAX {
            return Err(BtError::unsupported("zip 中央目录偏移超出 ZIP32 上限，已停止打包"));
        }
        if self.entries.len() > u16::MAX as usize {
            return Err(BtError::unsupported("zip 条目数超出 ZIP32 上限，已停止打包"));
        }
        for entry in &self.entries {
            self.file.write_all(&0x0201_4b50u32.to_le_bytes())?;
            self.file.write_all(&20u16.to_le_bytes())?;
            self.file.write_all(&20u16.to_le_bytes())?;
            self.file.write_all(&0x0800u16.to_le_bytes())?;
            self.file.write_all(&8u16.to_le_bytes())?;
            self.file.write_all(&0x0021u16.to_le_bytes())?;
            self.file.write_all(&0x0000u16.to_le_bytes())?;
            self.file.write_all(&entry.crc.to_le_bytes())?;
            self.file.write_all(&entry.compressed.to_le_bytes())?;
            self.file.write_all(&entry.uncompressed.to_le_bytes())?;
            self.file.write_all(&(entry.name.len() as u16).to_le_bytes())?;
            self.file.write_all(&0u16.to_le_bytes())?;
            self.file.write_all(&0u16.to_le_bytes())?;
            self.file.write_all(&0u16.to_le_bytes())?;
            self.file.write_all(&0u16.to_le_bytes())?;
            self.file.write_all(&0u32.to_le_bytes())?;
            self.file.write_all(&entry.offset.to_le_bytes())?;
            self.file.write_all(&entry.name)?;
        }
        let end = self.file.stream_position()?;
        if end > ZIP32_MAX {
            return Err(BtError::unsupported("zip 总大小超出 ZIP32 上限，已停止打包"));
        }
        let count = self.entries.len() as u16;
        self.file.write_all(&0x0605_4b50u32.to_le_bytes())?;
        self.file.write_all(&0u16.to_le_bytes())?;
        self.file.write_all(&0u16.to_le_bytes())?;
        self.file.write_all(&count.to_le_bytes())?;
        self.file.write_all(&count.to_le_bytes())?;
        self.file.write_all(&((end - start) as u32).to_le_bytes())?;
        self.file.write_all(&(start as u32).to_le_bytes())?;
        self.file.write_all(&0u16.to_le_bytes())?;
        self.file.sync_all()?;
        Ok(())
    }
}

fn collect_files(root: &Path, current: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> Result<(), BtError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            out.push((path, relative));
        }
    }
    Ok(())
}

fn streaming_crc32(reader: &mut File) -> Result<u32, BtError> {
    let mut crc = 0xffff_ffffu32;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        crc = crc32_update(crc, &buffer[..read]);
    }
    Ok(!crc)
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    crc
}

pub fn crc32(data: &[u8]) -> u32 {
    !crc32_update(0xffff_ffff, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let mut path = std::env::temp_dir();
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0);
            path.push(format!(
                "bt-package-{tag}-{}-{now}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn folder_zip_contains_relative_entries_and_round_trips() {
        let dir = TempDir::new("zip");
        let folder = dir.0.join("my_folder");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("a.txt"), b"hello").unwrap();
        std::fs::write(folder.join("sub/b.txt"), b"world").unwrap();
        let target = dir.0.join("out.zip");
        package_folder(&folder, &target).unwrap();
        let bytes = std::fs::read(&target).unwrap();
        assert_eq!(&bytes[..4], &0x0403_4b50u32.to_le_bytes());
        assert!(bytes.windows(5).any(|window| window == b"a.txt"));
        assert!(bytes.windows(9).any(|window| window == b"sub/b.txt"));
        assert_eq!(
            &bytes[bytes.len() - 22..bytes.len() - 18],
            &0x0605_4b50u32.to_le_bytes()
        );
        let entries = read_zip_entries(&bytes);
        assert_eq!(entries.len(), 2);
        for (name, content) in entries {
            let expected = match name.as_str() {
                "a.txt" => b"hello".to_vec(),
                "sub/b.txt" => b"world".to_vec(),
                other => panic!("unexpected entry {other}"),
            };
            assert_eq!(content, expected);
        }
    }

    #[test]
    fn single_file_zip_uses_basename() {
        let dir = TempDir::new("filezip");
        let source = dir.0.join("photo.jpg");
        std::fs::write(&source, b"jpeg").unwrap();
        let target = dir.0.join("out.zip");
        package_file(&source, &target).unwrap();
        let entries = read_zip_entries(&std::fs::read(&target).unwrap());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "photo.jpg");
        assert_eq!(entries[0].1, b"jpeg");
    }

    #[test]
    fn pending_store_creates_owned_text_and_zip() {
        let dir = TempDir::new("pending");
        let store = PendingPackageStore::new(dir.0.join("pending"));
        let text = store.create_text_file("你好", "note.txt").unwrap();
        assert!(text.owned);
        assert_eq!(text.display_name, "note.txt");
        assert_eq!(std::fs::read(&text.path).unwrap(), "你好".as_bytes());
        assert!(store.is_owned(&text.path));
        assert!(store.delete(&text.path));
        assert!(!text.path.exists());
        assert!(!store.delete(Path::new("/etc/passwd")));

        let source = dir.0.join("a.bin");
        std::fs::write(&source, b"data").unwrap();
        let zipped = store.create_zip_from_file(&source).unwrap();
        assert!(zipped.display_name.ends_with("a.bin.zip"));
        assert!(zipped.path.exists());
    }

    #[test]
    fn missing_folder_is_reported() {
        let error = plan_folder_package(Path::new("/nonexistent/bt-pack-folder")).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::NotFound);
    }

    fn sparse_file(path: &Path, size: u64) {
        let file = File::create(path).unwrap();
        file.set_len(size).unwrap();
    }

    #[test]
    fn zip32_size_boundaries_accept_below_and_reject_at_limit() {
        const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;
        let dir = TempDir::new("zip32-limits");
        let below = dir.0.join("below.bin");
        let at = dir.0.join("at.bin");
        sparse_file(&below, FOUR_GIB - 1);
        sparse_file(&at, FOUR_GIB);

        let items = collect_zip_items(std::slice::from_ref(&below), &ZipMode::File).unwrap();
        assert!(check_zip32_limits(&items).is_ok(), "4 GiB 减 1 字节应允许打包");

        let items = collect_zip_items(std::slice::from_ref(&at), &ZipMode::File).unwrap();
        let error = check_zip32_limits(&items).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
        assert!(error.message().contains("4 GiB"));
    }

    #[test]
    fn file_at_4gib_is_rejected_without_artifact() {
        const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;
        let dir = TempDir::new("zip32-file");
        let source = dir.0.join("huge.bin");
        sparse_file(&source, FOUR_GIB);
        let target = dir.0.join("out.zip");
        let error = package_file(&source, &target).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
        assert!(!target.exists(), "超限不得产出 zip 产物");
    }

    #[test]
    fn folder_cumulative_size_at_4gib_is_rejected_without_artifact() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let dir = TempDir::new("zip32-folder");
        let folder = dir.0.join("big_folder");
        std::fs::create_dir_all(&folder).unwrap();
        sparse_file(&folder.join("a.bin"), 3 * GIB);
        sparse_file(&folder.join("b.bin"), GIB);
        let target = dir.0.join("folder.zip");
        let error = package_folder(&folder, &target).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
        assert!(
            error.message().contains("累计"),
            "错误需说明累计大小：{}",
            error.message()
        );
        assert!(!target.exists(), "超限不得产出 zip 产物");
    }

    #[test]
    fn folder_precheck_reports_limit_without_artifact() {
        const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;
        let dir = TempDir::new("zip32-precheck");
        let folder = dir.0.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("small.txt"), b"ok").unwrap();
        assert!(check_folder_zip32(&folder).is_ok());

        sparse_file(&folder.join("big.bin"), FOUR_GIB);
        let error = check_folder_zip32(&folder).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
        assert!(error.message().contains("4 GiB"));
        assert!(!dir.0.join("folder.zip").exists(), "预检不得写产物");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_zip_source_maps_to_permission() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("zip-permission");
        let source = dir.0.join("secret.bin");
        std::fs::write(&source, b"secret").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&source).is_ok() {
            std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
            return;
        }
        let target = dir.0.join("out.zip");
        let error = package_file(&source, &target).unwrap_err();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Permission);
        assert!(!target.exists());
    }

    #[test]
    fn failed_zip_removes_partial_target() {
        let dir = TempDir::new("zip-cleanup");
        let source = dir.0.join("not-a-file");
        std::fs::create_dir_all(&source).unwrap();
        let target = dir.0.join("out.zip");
        assert!(write_zip(&target, std::slice::from_ref(&source), ZipMode::File).is_err());
        assert!(!target.exists(), "打包失败后不得残留半成品");
    }

    fn read_zip_entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 30 <= bytes.len() {
            let signature = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            if signature != 0x0403_4b50 {
                break;
            }
            let method = u16::from_le_bytes(bytes[pos + 8..pos + 10].try_into().unwrap());
            let compressed = u32::from_le_bytes(bytes[pos + 18..pos + 22].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes(bytes[pos + 26..pos + 28].try_into().unwrap()) as usize;
            let extra_len = u16::from_le_bytes(bytes[pos + 28..pos + 30].try_into().unwrap()) as usize;
            let name_start = pos + 30;
            let data_start = name_start + name_len + extra_len;
            let name = String::from_utf8(bytes[name_start..name_start + name_len].to_vec()).unwrap();
            let compressed_data = &bytes[data_start..data_start + compressed];
            let content = if method == 8 {
                let mut decoder = flate2::read::DeflateDecoder::new(compressed_data);
                let mut buffer = Vec::new();
                decoder.read_to_end(&mut buffer).unwrap();
                buffer
            } else {
                compressed_data.to_vec()
            };
            out.push((name, content));
            pos = data_start + compressed;
        }
        out
    }
}
