use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use bt_core::{hex_lower, paths, sha256_file, BtError, TransferId};

use crate::protocol::{Done, Hello};
use crate::resume::ResumeSidecar;

pub const PARTIAL_SUFFIX: &str = ".btpart";
pub const META_SUFFIX: &str = ".meta";
/// 接收半成品的保留期，与发送 journal 的 7 天口径一致。
pub const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 接收端落盘：半成品与二进制 sidecar、SHA-256 校验、重名自动改名。
#[derive(Debug, Clone)]
pub struct FileSink {
    dir: PathBuf,
}

impl FileSink {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, BtError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn partial_path(&self, hello: &Hello) -> Result<PathBuf, BtError> {
        let name = paths::sanitize_file_name(&hello.file_name);
        Ok(self.dir.join(format!("{name}{PARTIAL_SUFFIX}")))
    }

    pub fn meta_path(&self, hello: &Hello) -> Result<PathBuf, BtError> {
        let part = self.partial_path(hello)?;
        let name = format!(
            "{}{}",
            part.file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default(),
            META_SUFFIX
        );
        Ok(part.with_file_name(name))
    }

    fn expected(&self, hello: &Hello) -> Result<TransferId, BtError> {
        hello.expected_digest()
    }

    /// 打开接收：命中半成品返回已落盘偏移，否则清零重来。
    pub fn open(&self, hello: &Hello) -> Result<u64, BtError> {
        let total = hello.file_size as u64;
        let expected = self.expected(hello)?;
        let part = self.partial_path(hello)?;
        let meta = self.meta_path(hello)?;

        if part.is_file() {
            if let Ok(sidecar) = ResumeSidecar::read_from(&meta) {
                let length = std::fs::metadata(&part)?.len();
                if sidecar.matches(&expected, total) && length <= total {
                    ResumeSidecar::new(expected, length, total).write_to(&meta)?;
                    return Ok(length);
                }
            }
        }

        remove_if_exists(&part)?;
        remove_if_exists(&meta)?;
        File::create(&part)?.sync_all()?;
        ResumeSidecar::new(expected, 0, total).write_to(&meta)?;
        Ok(0)
    }

    /// 追加写入并落盘；偏移必须等于当前长度。
    pub fn write(&self, hello: &Hello, offset: u64, data: &[u8]) -> Result<(), BtError> {
        let total = hello.file_size as u64;
        if offset > total || data.len() as u64 > total - offset {
            return Err(BtError::protocol(format!(
                "写入越过文件末尾 偏移 {offset} 长度 {} 总长 {total}",
                data.len()
            )));
        }
        let part = self.partial_path(hello)?;
        let mut file = OpenOptions::new().append(true).open(&part)?;
        let length = file.metadata()?.len();
        if length != offset {
            return Err(BtError::protocol(format!("偏移不一致 期望 {length} 收到 {offset}")));
        }
        file.write_all(data)?;
        file.flush()?;
        file.sync_data()?;
        let meta = self.meta_path(hello)?;
        ResumeSidecar::new(self.expected(hello)?, offset + data.len() as u64, total).write_to(&meta)?;
        Ok(())
    }

    /// 完成：校验 SHA-256 后改名到正式文件；不匹配时清空半成品。
    pub fn complete(&self, hello: &Hello) -> Result<Done, BtError> {
        Ok(self.complete_with_path(hello)?.0)
    }

    /// 完成并返回实际落盘路径；校验失败时路径为空。
    pub fn complete_with_path(&self, hello: &Hello) -> Result<(Done, Option<PathBuf>), BtError> {
        let part = self.partial_path(hello)?;
        let meta = self.meta_path(hello)?;
        if !part.is_file() {
            if hello.file_size == 0 {
                File::create(&part)?.sync_all()?;
            } else {
                return Err(BtError::protocol("半成品文件不存在"));
            }
        }
        let actual = sha256_file(&part)?;
        let actual_hex = hex_lower(&actual);
        let expected = self.expected(hello)?;
        if actual == expected {
            let final_path = self.planned_path(hello)?;
            std::fs::rename(&part, &final_path)?;
            remove_if_exists(&meta)?;
            Ok((Done::new(true, actual_hex), Some(final_path)))
        } else {
            remove_if_exists(&part)?;
            remove_if_exists(&meta)?;
            Ok((Done::new(false, actual_hex), None))
        }
    }

    /// 中止：delete_partial 为真时删除半成品，否则保留并标记暂停。
    pub fn abort(&self, hello: &Hello, delete_partial: bool) -> Result<(), BtError> {
        let part = self.partial_path(hello)?;
        let meta = self.meta_path(hello)?;
        if delete_partial {
            remove_if_exists(&part)?;
            remove_if_exists(&meta)?;
            return Ok(());
        }
        if let Ok(length) = std::fs::metadata(&part).map(|m| m.len()) {
            let mut sidecar = ResumeSidecar::new(self.expected(hello)?, length, hello.file_size as u64);
            sidecar.flags |= crate::resume::FLAG_PAUSED;
            sidecar.write_to(&meta)?;
        }
        Ok(())
    }

    /// 正式文件的候选路径；同名冲突时生成 stem (n).ext。
    pub fn planned_path(&self, hello: &Hello) -> Result<PathBuf, BtError> {
        let cleaned = paths::sanitize_file_name(&hello.file_name);
        Ok(paths::unique_child(&self.dir, &cleaned))
    }

    /// 清理超过保留期的半成品与孤儿 sidecar，返回清理数量。
    pub fn clean_stale(&self, older_than: Duration) -> Result<usize, BtError> {
        clean_stale_dir(&self.dir, older_than)
    }
}

pub fn clean_stale_dir(dir: &Path, older_than: Duration) -> Result<usize, BtError> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut removed = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.ends_with(PARTIAL_SUFFIX) {
            continue;
        }
        let metadata = entry.metadata()?;
        let age = metadata
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok());
        if age.is_some_and(|age| age > older_than) {
            remove_if_exists(&path)?;
            remove_if_exists(&PathBuf::from(format!("{}{}", path.display(), META_SUFFIX)))?;
            removed += 1;
        }
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.ends_with(&format!("{PARTIAL_SUFFIX}{META_SUFFIX}")) {
            continue;
        }
        let base = name.trim_end_matches(META_SUFFIX).to_string();
        if !dir.join(base).is_file() {
            remove_if_exists(&path)?;
        }
    }
    Ok(removed)
}

fn remove_if_exists(path: &Path) -> Result<(), BtError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::sha256;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let mut path = std::env::temp_dir();
            let now = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|v| v.as_nanos())
                .unwrap_or(0);
            path.push(format!(
                "bt-sink-{tag}-{}-{now}-{}",
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

    fn hello(name: &str, content: &[u8]) -> Hello {
        Hello::new(name, content.len() as i64, hex_lower(&sha256(content)), 4096)
    }

    #[test]
    fn write_complete_renames_with_zero_overhead() {
        let dir = TempDir::new("complete");
        let sink = FileSink::new(&dir.0).unwrap();
        let content = b"payload-bytes".to_vec();
        let hello = hello("a.bin", &content);
        assert_eq!(sink.open(&hello).unwrap(), 0);
        sink.write(&hello, 0, &content).unwrap();
        let done = sink.complete(&hello).unwrap();
        assert!(done.ok);
        let final_path = dir.0.join("a.bin");
        assert_eq!(std::fs::read(&final_path).unwrap(), content);
        assert!(!sink.partial_path(&hello).unwrap().exists());
        assert!(!sink.meta_path(&hello).unwrap().exists());
        assert_eq!(std::fs::metadata(&final_path).unwrap().len() as usize, content.len());
        eprintln!(
            "MEASURE receive_source_bytes={} final_bytes={} extra={}",
            content.len(),
            std::fs::metadata(&final_path).unwrap().len(),
            std::fs::metadata(&final_path).unwrap().len() as usize - content.len()
        );
    }

    #[test]
    fn reopen_with_same_transfer_returns_resume_offset() {
        let dir = TempDir::new("resume");
        let sink = FileSink::new(&dir.0).unwrap();
        let content = vec![9u8; 100];
        let hello = hello("big.bin", &content);
        assert_eq!(sink.open(&hello).unwrap(), 0);
        sink.write(&hello, 0, &content[..40]).unwrap();
        assert_eq!(sink.open(&hello).unwrap(), 40);
        let meta = sink.meta_path(&hello).unwrap();
        assert!(std::fs::metadata(&meta).unwrap().len() as usize <= crate::resume::SIDECAR_LIMIT);
    }

    #[test]
    fn different_transfer_starts_over() {
        let dir = TempDir::new("other");
        let sink = FileSink::new(&dir.0).unwrap();
        let first = hello("a.bin", b"12345");
        sink.open(&first).unwrap();
        sink.write(&first, 0, b"123").unwrap();
        let second = hello("a.bin", b"abcde");
        assert_eq!(sink.open(&second).unwrap(), 0);
    }

    #[test]
    fn hash_mismatch_deletes_partial() {
        let dir = TempDir::new("mismatch");
        let sink = FileSink::new(&dir.0).unwrap();
        let content = b"bad";
        let mut hello = hello("a.bin", content);
        hello.expected_sha256 = "00".repeat(32);
        sink.open(&hello).unwrap();
        sink.write(&hello, 0, content).unwrap();
        let done = sink.complete(&hello).unwrap();
        assert!(!done.ok);
        assert!(!sink.partial_path(&hello).unwrap().exists());
        assert!(!sink.meta_path(&hello).unwrap().exists());
        assert!(!dir.0.join("a.bin").exists());
    }

    #[test]
    fn duplicate_name_gets_numbered_suffix() {
        let dir = TempDir::new("dup");
        std::fs::write(dir.0.join("a.bin"), b"old").unwrap();
        let sink = FileSink::new(&dir.0).unwrap();
        let content = b"new";
        let hello = hello("a.bin", content);
        sink.open(&hello).unwrap();
        sink.write(&hello, 0, content).unwrap();
        assert!(sink.complete(&hello).unwrap().ok);
        assert_eq!(std::fs::read(dir.0.join("a (1).bin")).unwrap(), content);
        assert_eq!(std::fs::read(dir.0.join("a.bin")).unwrap(), b"old");
    }

    #[test]
    fn abort_keeps_partial_for_resume() {
        let dir = TempDir::new("abort");
        let sink = FileSink::new(&dir.0).unwrap();
        let content = vec![1u8; 50];
        let hello = hello("keep.bin", &content);
        sink.open(&hello).unwrap();
        sink.write(&hello, 0, &content[..20]).unwrap();
        sink.abort(&hello, false).unwrap();
        assert!(sink.partial_path(&hello).unwrap().exists());
        assert_eq!(sink.open(&hello).unwrap(), 20);
        sink.abort(&hello, true).unwrap();
        assert!(!sink.partial_path(&hello).unwrap().exists());
    }

    #[test]
    fn stale_cleanup_removes_old_partials_only() {
        let dir = TempDir::new("stale");
        let old = dir.0.join("old.bin.btpart");
        std::fs::write(&old, b"x").unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        let handle = File::options().write(true).open(&old).unwrap();
        handle
            .set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        std::fs::write(dir.0.join("old.bin.btpart.meta"), b"y").unwrap();
        std::fs::write(dir.0.join("new.bin.btpart"), b"z").unwrap();
        let removed = clean_stale_dir(&dir.0, STALE_AFTER).unwrap();
        assert_eq!(removed, 1);
        assert!(!old.exists());
        assert!(!dir.0.join("old.bin.btpart.meta").exists());
        assert!(dir.0.join("new.bin.btpart").exists());
    }

    #[test]
    fn write_rejects_wrong_offset_and_overrun() {
        let dir = TempDir::new("bounds");
        let sink = FileSink::new(&dir.0).unwrap();
        let content = b"12345";
        let hello = hello("a.bin", content);
        sink.open(&hello).unwrap();
        assert!(sink.write(&hello, 1, b"1").is_err());
        assert!(sink.write(&hello, 0, b"123456").is_err());
        sink.write(&hello, 0, b"12345").unwrap();
        assert!(sink.write(&hello, 5, b"x").is_err());
    }

    #[test]
    fn empty_file_completes() {
        let dir = TempDir::new("empty");
        let sink = FileSink::new(&dir.0).unwrap();
        let hello = hello("empty.bin", b"");
        assert_eq!(sink.open(&hello).unwrap(), 0);
        let done = sink.complete(&hello).unwrap();
        assert!(done.ok);
        assert_eq!(std::fs::metadata(dir.0.join("empty.bin")).unwrap().len(), 0);
    }
}
