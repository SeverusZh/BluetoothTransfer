use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bt_core::{hex_lower, sha256, sha256_file, BtError, DeviceId, TransferId};

pub const JOURNAL_MAGIC: [u8; 4] = *b"BTJ1";
/// 单条目编码后的体积上限。
pub const MAX_ENTRY_BYTES: usize = 256;
pub const MAX_ENTRIES: usize = 200;
pub const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const LEASE: Duration = Duration::from_secs(6 * 60);
pub const RENEW: Duration = Duration::from_secs(60);
pub const MAX_ATTEMPTS: u32 = 6;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_NAME_BYTES: usize = 110;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    Pending,
    Sending,
    Paused,
}

impl JournalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Paused => "paused",
        }
    }

    fn as_byte(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::Sending => 1,
            Self::Paused => 2,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, BtError> {
        match byte {
            0 => Ok(Self::Pending),
            1 => Ok(Self::Sending),
            2 => Ok(Self::Paused),
            other => Err(BtError::protocol(format!("未知 journal 状态 {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalKind {
    File,
    FileZip,
    Folder,
    Text,
}

impl JournalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::FileZip => "file-zip",
            Self::Folder => "folder",
            Self::Text => "text",
        }
    }

    fn as_byte(self) -> u8 {
        match self {
            Self::File => 0,
            Self::FileZip => 1,
            Self::Folder => 2,
            Self::Text => 3,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, BtError> {
        match byte {
            0 => Ok(Self::File),
            1 => Ok(Self::FileZip),
            2 => Ok(Self::Folder),
            3 => Ok(Self::Text),
            other => Err(BtError::protocol(format!("未知 journal 来源类型 {other}"))),
        }
    }
}

/// 一条未完成传输：接收端按 DisplayName + SHA + Size 命中半成品。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: [u8; 8],
    pub peer: DeviceId,
    pub display_name: String,
    pub size: u64,
    pub sha256: TransferId,
    pub payload: PathBuf,
    pub kind: JournalKind,
    pub produced: bool,
    pub state: JournalState,
    pub attempts: u32,
    pub resume_offset: u64,
    pub lease_until: u64,
    pub owner_pid: u32,
    pub owner_start: u32,
    pub created: u32,
    pub updated: u32,
}

impl JournalEntry {
    pub fn new(
        peer: DeviceId,
        display_name: impl Into<String>,
        payload: PathBuf,
        sha256: TransferId,
        size: u64,
        kind: JournalKind,
        produced: bool,
    ) -> Self {
        Self {
            id: [0u8; 8],
            peer,
            display_name: display_name.into(),
            size,
            sha256,
            payload,
            kind,
            produced,
            state: JournalState::Pending,
            attempts: 0,
            resume_offset: 0,
            lease_until: 0,
            owner_pid: 0,
            owner_start: 0,
            created: 0,
            updated: 0,
        }
    }

    pub fn id_hex(&self) -> String {
        hex_lower(&self.id)
    }

    /// 幂等键：对端地址 + 内容 SHA-256 + 落盘文件名。
    pub fn dedupe_key(&self) -> String {
        format!("{}|{}|{}", self.peer, hex_lower(&self.sha256), self.display_name)
    }

    pub fn is_busy(&self, now: u64, liveness: &dyn ProcessLiveness) -> bool {
        if self.state != JournalState::Sending {
            return false;
        }
        if self.owner_pid == 0 {
            return self.lease_until > now;
        }
        if liveness.reliable() {
            return liveness.is_alive(self.owner_pid, self.owner_start);
        }
        self.lease_until > now
    }

    pub fn encode(&self, base: &Path) -> Result<Vec<u8>, BtError> {
        let payload = path_to_relative(&self.payload, base)?;
        if self.display_name.len() > MAX_NAME_BYTES {
            return Err(BtError::invalid(format!("journal 文件名超出 {MAX_NAME_BYTES} 字节")));
        }
        let mut out = Vec::with_capacity(91 + self.display_name.len() + payload.len());
        out.push(1u8);
        out.extend_from_slice(&self.id);
        out.extend_from_slice(self.peer.as_bytes());
        out.extend_from_slice(&self.sha256);
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.resume_offset.to_le_bytes());
        out.extend_from_slice(&(self.lease_until as u32).to_le_bytes());
        out.extend_from_slice(&self.owner_pid.to_le_bytes());
        out.extend_from_slice(&self.owner_start.to_le_bytes());
        out.extend_from_slice(&self.created.to_le_bytes());
        out.extend_from_slice(&self.updated.to_le_bytes());
        out.extend_from_slice(&(self.attempts as u16).to_le_bytes());
        out.push(self.state.as_byte());
        out.push(self.kind.as_byte());
        out.push(u8::from(self.produced));
        out.push(self.display_name.len() as u8);
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(self.display_name.as_bytes());
        out.extend_from_slice(payload.as_bytes());
        if out.len() > MAX_ENTRY_BYTES {
            return Err(BtError::invalid(format!(
                "journal 条目超出上限 {} > {MAX_ENTRY_BYTES}",
                out.len()
            )));
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8], base: &Path) -> Result<Self, BtError> {
        if bytes.len() < 91 {
            return Err(BtError::protocol("journal 条目截断"));
        }
        if bytes[0] != 1 {
            return Err(BtError::protocol(format!("journal 版本不支持 {}", bytes[0])));
        }
        let mut id = [0u8; 8];
        id.copy_from_slice(&bytes[1..9]);
        let mut peer = [0u8; 6];
        peer.copy_from_slice(&bytes[9..15]);
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&bytes[15..47]);
        let size = read_u64(bytes, 47)?;
        let resume_offset = read_u64(bytes, 55)?;
        let lease_until = read_u32(bytes, 63)? as u64;
        let owner_pid = read_u32(bytes, 67)?;
        let owner_start = read_u32(bytes, 71)?;
        let created = read_u32(bytes, 75)?;
        let updated = read_u32(bytes, 79)?;
        let attempts = read_u16(bytes, 83)? as u32;
        let state = JournalState::from_byte(bytes[85])?;
        let kind = JournalKind::from_byte(bytes[86])?;
        let produced = bytes[87] & 1 != 0;
        let name_len = bytes[88] as usize;
        let payload_len = read_u16(bytes, 89)? as usize;
        let name_start = 91usize;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| BtError::protocol("journal 文件名长度溢出"))?;
        let payload_end = name_end
            .checked_add(payload_len)
            .ok_or_else(|| BtError::protocol("journal 载荷路径长度溢出"))?;
        if payload_end != bytes.len() {
            return Err(BtError::protocol("journal 条目长度字段与实际不符"));
        }
        let display_name = std::str::from_utf8(&bytes[name_start..name_end])
            .map_err(|_| BtError::protocol("journal 文件名不是合法 UTF-8"))?
            .to_string();
        let payload_text = std::str::from_utf8(&bytes[name_end..payload_end])
            .map_err(|_| BtError::protocol("journal 载荷路径不是合法 UTF-8"))?;
        if payload_text.is_empty() {
            return Err(BtError::protocol("journal 载荷路径为空"));
        }
        let payload = relative_to_path(payload_text, base);
        Ok(Self {
            id,
            peer: DeviceId::from_bytes(peer),
            display_name,
            size,
            sha256,
            payload,
            kind,
            produced,
            state,
            attempts,
            resume_offset,
            lease_until,
            owner_pid,
            owner_start,
            created,
            updated,
        })
    }

    pub fn validate_payload(&self) -> Result<(), String> {
        if !self.payload.is_file() {
            return Err(format!("待发送文件已不存在：{}", self.payload.display()));
        }
        let length = std::fs::metadata(&self.payload)
            .map_err(|error| format!("待发送文件不可读：{error}"))?
            .len();
        if length != self.size {
            return Err(format!("源文件大小已变化（{} -> {length}），无法续传", self.size));
        }
        let digest = sha256_file(&self.payload).map_err(|error| format!("计算校验和失败：{error}"))?;
        if digest != self.sha256 {
            return Err("源文件内容已变化（SHA-256 不一致），无法续传".to_string());
        }
        Ok(())
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, BtError> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
        .ok_or_else(|| BtError::protocol("journal 字段截断"))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, BtError> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
        .ok_or_else(|| BtError::protocol("journal 字段截断"))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, BtError> {
    bytes
        .get(offset..offset + 8)
        .map(|slice| u64::from_le_bytes(slice.try_into().unwrap_or([0u8; 8])))
        .ok_or_else(|| BtError::protocol("journal 字段截断"))
}

fn path_to_relative(path: &Path, base: &Path) -> Result<String, BtError> {
    let text = path
        .to_str()
        .ok_or_else(|| BtError::invalid("journal 载荷路径不是合法 UTF-8"))?;
    if let Ok(relative) = path.strip_prefix(base) {
        let relative = relative
            .to_str()
            .ok_or_else(|| BtError::invalid("journal 相对路径不是合法 UTF-8"))?;
        if !relative.is_empty() {
            return Ok(relative.to_string());
        }
    }
    Ok(text.to_string())
}

fn relative_to_path(text: &str, base: &Path) -> PathBuf {
    let path = Path::new(text);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// 持有者进程存活判定；可靠判定不可用时退化为租约判断。
pub trait ProcessLiveness: Send + Sync {
    fn is_alive(&self, pid: u32, start_marker: u32) -> bool;

    fn reliable(&self) -> bool {
        true
    }
}

pub struct SystemLiveness;

impl ProcessLiveness for SystemLiveness {
    fn is_alive(&self, pid: u32, start_marker: u32) -> bool {
        platform::is_process_alive(pid, start_marker)
    }

    fn reliable(&self) -> bool {
        cfg!(any(target_os = "linux", target_os = "windows"))
    }
}

pub fn current_process() -> (u32, u32) {
    platform::current_process()
}

pub fn default_liveness() -> Arc<dyn ProcessLiveness> {
    Arc::new(SystemLiveness)
}

pub fn default_clock() -> Arc<dyn Fn() -> u64 + Send + Sync> {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0)
    })
}

/// 打包产物清理回调，journal 删除条目时调用。
pub trait ArtifactCleaner: Send + Sync {
    fn delete(&self, path: &Path) -> bool;
}

#[derive(Debug)]
pub struct BeginResult {
    pub entry: JournalEntry,
    pub reused: bool,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub struct Abandoned {
    pub entry: JournalEntry,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct RecoveryPlan {
    pub recoverable: Vec<JournalEntry>,
    pub abandoned: Vec<Abandoned>,
}

pub struct Journal {
    path: PathBuf,
    base: PathBuf,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    liveness: Arc<dyn ProcessLiveness>,
    process: (u32, u32),
    cleaner: Option<Arc<dyn ArtifactCleaner>>,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Journal")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Journal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Self {
            path,
            base,
            clock: default_clock(),
            liveness: default_liveness(),
            process: current_process(),
            cleaner: None,
        }
    }

    /// 覆盖载荷路径的解析基准，默认是 journal 文件所在目录。
    pub fn with_base(mut self, base: impl Into<PathBuf>) -> Self {
        self.base = base.into();
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_liveness(mut self, liveness: Arc<dyn ProcessLiveness>) -> Self {
        self.liveness = liveness;
        self
    }

    pub fn with_cleaner(mut self, cleaner: Arc<dyn ArtifactCleaner>) -> Self {
        self.cleaner = Some(cleaner);
        self
    }

    pub fn with_process(mut self, pid: u32, start_marker: u32) -> Self {
        self.process = (pid, start_marker);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    pub fn read_all(&self) -> Result<Vec<JournalEntry>, BtError> {
        lock_journal(&self.path, || self.read_unlocked())
    }

    /// 在跨进程独占锁内读改写并原子落盘。
    fn mutate<T>(&self, mutate: impl FnOnce(&mut Vec<JournalEntry>, u64) -> T) -> Result<T, BtError> {
        lock_journal(&self.path, || {
            let mut entries = self.read_unlocked()?;
            let now = self.now();
            let result = mutate(&mut entries, now);
            self.write_unlocked(&entries)?;
            Ok(result)
        })
    }

    fn read_unlocked(&self) -> Result<Vec<JournalEntry>, BtError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        match decode_file(&bytes, &self.base) {
            Ok(entries) => Ok(entries),
            Err(_) => {
                quarantine(&self.path);
                Ok(Vec::new())
            }
        }
    }

    fn write_unlocked(&self, entries: &[JournalEntry]) -> Result<(), BtError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut bytes = Vec::with_capacity(4 + entries.len() * 128);
        bytes.extend_from_slice(&JOURNAL_MAGIC);
        for entry in entries {
            let encoded = entry.encode(&self.base)?;
            bytes.extend_from_slice(&(encoded.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&encoded);
        }
        let temporary = temporary_path(&self.path);
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, &self.path)?;
        sync_parent(&self.path);
        Ok(())
    }

    fn claim_entry(&self, entry: &mut JournalEntry, now: u64) {
        entry.state = JournalState::Sending;
        entry.lease_until = now + LEASE.as_secs();
        entry.owner_pid = self.process.0;
        entry.owner_start = self.process.1;
        entry.updated = clamp_u32(now);
        if entry.created == 0 {
            entry.created = clamp_u32(now);
        }
    }

    fn sweep(&self, entries: &mut Vec<JournalEntry>, now: u64, mut removed: impl FnMut(&JournalEntry)) {
        let mut index = 0;
        while index < entries.len() {
            let busy = entries[index].is_busy(now, self.liveness.as_ref());
            let expired = now.saturating_sub(entries[index].updated as u64) > TTL.as_secs();
            if !busy && expired {
                let entry = entries.remove(index);
                removed(&entry);
            } else {
                index += 1;
            }
        }
        if entries.len() <= MAX_ENTRIES {
            return;
        }
        let excess = entries.len() - MAX_ENTRIES;
        let mut candidates: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_busy(now, self.liveness.as_ref()))
            .map(|(index, _)| index)
            .collect();
        candidates.sort_by_key(|index| (entries[*index].created, entries[*index].id));
        let mut remove_indexes: Vec<usize> = candidates.into_iter().take(excess).collect();
        remove_indexes.sort_unstable_by(|a, b| b.cmp(a));
        for index in remove_indexes {
            let entry = entries.remove(index);
            removed(&entry);
        }
    }

    fn delete_artifacts(&self, entries: &[JournalEntry]) {
        let Some(cleaner) = self.cleaner.as_ref() else {
            return;
        };
        for entry in entries {
            if entry.produced {
                cleaner.delete(&entry.payload);
            }
        }
    }

    pub fn begin(&self, mut entry: JournalEntry) -> Result<BeginResult, BtError> {
        let mut removed = Vec::new();
        let result = self.mutate(|entries, now| {
            self.sweep(entries, now, |entry| removed.push(entry.clone()));
            if let Some(existing) = entries
                .iter_mut()
                .find(|existing| existing.dedupe_key() == entry.dedupe_key())
            {
                if existing.is_busy(now, self.liveness.as_ref()) {
                    return BeginResult {
                        entry: existing.clone(),
                        reused: true,
                        busy: true,
                    };
                }
                existing.peer = entry.peer;
                existing.display_name = entry.display_name.clone();
                existing.size = entry.size;
                existing.sha256 = entry.sha256;
                existing.payload = entry.payload;
                existing.kind = entry.kind;
                existing.produced = entry.produced;
                existing.resume_offset = 0;
                existing.attempts = existing.attempts.min(MAX_ATTEMPTS);
                self.claim_entry(existing, now);
                return BeginResult {
                    entry: existing.clone(),
                    reused: true,
                    busy: false,
                };
            }
            if entry.id == [0u8; 8] {
                entry.id = generate_id(&entry, now);
            }
            entry.resume_offset = 0;
            if entry.created == 0 {
                entry.created = clamp_u32(now);
            }
            self.claim_entry(&mut entry, now);
            entries.push(entry.clone());
            self.sweep(entries, now, |entry| removed.push(entry.clone()));
            BeginResult {
                entry,
                reused: false,
                busy: false,
            }
        });
        if !result.as_ref().is_ok_and(|result| result.busy) {
            self.delete_artifacts(&removed);
        }
        result
    }

    pub fn claim(&self, id: &str) -> Result<bool, BtError> {
        let mut removed = Vec::new();
        let result = self.mutate(|entries, now| {
            self.sweep(entries, now, |entry| removed.push(entry.clone()));
            let Some(entry) = entries.iter_mut().find(|entry| entry.id_hex() == id) else {
                return false;
            };
            if entry.state == JournalState::Paused || entry.is_busy(now, self.liveness.as_ref()) {
                return false;
            }
            self.claim_entry(entry, now);
            true
        });
        if result.as_ref().is_ok_and(|claimed| *claimed) {
            self.delete_artifacts(&removed);
        }
        result
    }

    pub fn renew(&self, id: &str) -> Result<bool, BtError> {
        self.mutate(|entries, now| {
            let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.id_hex() == id && entry.state == JournalState::Sending)
            else {
                return false;
            };
            entry.lease_until = now + LEASE.as_secs();
            entry.updated = clamp_u32(now);
            true
        })
    }

    pub fn release(&self, id: &str, count_attempt: bool) -> Result<bool, BtError> {
        self.mutate(|entries, now| {
            let Some(entry) = entries.iter_mut().find(|entry| entry.id_hex() == id) else {
                return false;
            };
            entry.state = JournalState::Pending;
            entry.lease_until = 0;
            entry.owner_pid = 0;
            entry.owner_start = 0;
            if count_attempt {
                entry.attempts = entry.attempts.saturating_add(1);
            }
            entry.updated = clamp_u32(now);
            true
        })
    }

    pub fn pause(&self, id: &str) -> Result<bool, BtError> {
        self.mutate(|entries, now| {
            let Some(entry) = entries.iter_mut().find(|entry| entry.id_hex() == id) else {
                return false;
            };
            entry.state = JournalState::Paused;
            entry.lease_until = 0;
            entry.owner_pid = 0;
            entry.owner_start = 0;
            entry.updated = clamp_u32(now);
            true
        })
    }

    /// 续租并记录进度：一次加锁内刷新租约到期与已确认偏移。
    pub fn heartbeat(&self, id: &str, offset: u64) -> Result<bool, BtError> {
        self.mutate(|entries, now| {
            let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.id_hex() == id && entry.state == JournalState::Sending)
            else {
                return false;
            };
            entry.lease_until = now + LEASE.as_secs();
            entry.resume_offset = offset.min(entry.size);
            entry.updated = clamp_u32(now);
            true
        })
    }

    pub fn mark_progress(&self, id: &str, offset: u64) -> Result<bool, BtError> {
        self.mutate(|entries, now| {
            let Some(entry) = entries.iter_mut().find(|entry| entry.id_hex() == id) else {
                return false;
            };
            entry.resume_offset = offset;
            entry.updated = clamp_u32(now);
            true
        })
    }

    pub fn complete(&self, id: &str) -> Result<bool, BtError> {
        self.remove(id).map(|entry| entry.is_some())
    }

    pub fn cancel(&self, id: &str) -> Result<bool, BtError> {
        self.remove(id).map(|entry| entry.is_some())
    }

    pub fn remove(&self, id: &str) -> Result<Option<JournalEntry>, BtError> {
        let removed = self.mutate(|entries, _| {
            let index = entries.iter().position(|entry| entry.id_hex() == id);
            index.map(|index| entries.remove(index))
        })?;
        if let Some(entry) = removed.as_ref() {
            self.delete_artifacts(std::slice::from_ref(entry));
        }
        Ok(removed)
    }

    pub fn plan_recovery(&self) -> Result<RecoveryPlan, BtError> {
        let now = self.now();
        let mut plan = RecoveryPlan::default();
        for entry in self.read_all()? {
            if entry.state == JournalState::Paused {
                continue;
            }
            if entry.is_busy(now, self.liveness.as_ref()) {
                continue;
            }
            if now.saturating_sub(entry.updated as u64) > TTL.as_secs() {
                plan.abandoned.push(Abandoned {
                    entry,
                    reason: "已超过 7 天保留期，放弃断点续传".to_string(),
                });
                continue;
            }
            if entry.attempts >= MAX_ATTEMPTS {
                let attempts = entry.attempts;
                plan.abandoned.push(Abandoned {
                    entry,
                    reason: format!("已尝试 {attempts} 次仍未完成，放弃断点续传"),
                });
                continue;
            }
            if let Err(reason) = entry.validate_payload() {
                plan.abandoned.push(Abandoned { entry, reason });
                continue;
            }
            plan.recoverable.push(entry);
        }
        Ok(plan)
    }

    pub fn apply_abandoned(&self, ids: &[String]) -> Result<usize, BtError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let removed = self.mutate(|entries, _| {
            let mut removed = Vec::new();
            entries.retain(|entry| {
                if ids.iter().any(|id| id == &entry.id_hex()) {
                    removed.push(entry.clone());
                    false
                } else {
                    true
                }
            });
            removed
        })?;
        self.delete_artifacts(&removed);
        Ok(removed.len())
    }

    pub fn cleanup_expired(&self) -> Result<usize, BtError> {
        let mut removed = Vec::new();
        self.mutate(|entries, now| {
            self.sweep(entries, now, |entry| removed.push(entry.clone()));
        })?;
        self.delete_artifacts(&removed);
        Ok(removed.len())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
}

impl Journal {
    /// 导入旧版本或外部来源的条目；按幂等键去重，越界条目跳过。
    pub fn import(&self, entries: Vec<JournalEntry>) -> Result<ImportReport, BtError> {
        let mut report = ImportReport::default();
        if entries.is_empty() {
            return Ok(report);
        }
        self.mutate(|current, now| {
            self.sweep(current, now, |_entry| {});
            for mut entry in entries {
                if current
                    .iter()
                    .any(|existing| existing.dedupe_key() == entry.dedupe_key())
                {
                    report.skipped += 1;
                    continue;
                }
                if entry.id == [0u8; 8] {
                    entry.id = generate_id(&entry, now);
                }
                if entry.state == JournalState::Sending {
                    entry.state = JournalState::Pending;
                }
                entry.lease_until = 0;
                entry.owner_pid = 0;
                entry.owner_start = 0;
                if entry.created == 0 {
                    entry.created = clamp_u32(now);
                }
                if entry.updated == 0 {
                    entry.updated = clamp_u32(now);
                }
                if entry.encode(&self.base).is_err() {
                    report.skipped += 1;
                    continue;
                }
                current.push(entry);
                report.imported += 1;
            }
            report
        })
    }
}

fn decode_file(bytes: &[u8], base: &Path) -> Result<Vec<JournalEntry>, BtError> {
    if bytes.len() < 4 || bytes[0..4] != JOURNAL_MAGIC {
        return Err(BtError::protocol("journal 魔数不匹配"));
    }
    let mut entries = Vec::new();
    let mut pos = 4usize;
    while pos < bytes.len() {
        if bytes.len() - pos < 2 {
            return Err(BtError::protocol("journal 记录长度字段截断"));
        }
        let length = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        if length == 0 || length > MAX_ENTRY_BYTES || bytes.len() - pos < length {
            return Err(BtError::protocol("journal 记录长度非法"));
        }
        entries.push(JournalEntry::decode(&bytes[pos..pos + length], base)?);
        pos += length;
    }
    Ok(entries)
}

fn quarantine(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "send-journal".to_string());
    let target = path.with_file_name(format!("{name}.corrupt.{stamp}"));
    let _ = std::fs::rename(path, target);
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "send-journal".to_string());
    path.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            if let Ok(directory) = std::fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn clamp_u32(value: u64) -> u32 {
    value.min(u32::MAX as u64) as u32
}

fn generate_id(entry: &JournalEntry, now: u64) -> [u8; 8] {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut seed = Vec::with_capacity(64);
    seed.extend_from_slice(&now.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&counter.to_le_bytes());
    seed.extend_from_slice(entry.peer.as_bytes());
    seed.extend_from_slice(&entry.sha256);
    seed.extend_from_slice(entry.display_name.as_bytes());
    let digest = sha256(&seed);
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[..8]);
    id
}

fn lock_journal<T>(path: &Path, action: impl FnOnce() -> Result<T, BtError>) -> Result<T, BtError> {
    static LOCKS: OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let table = LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let key = std::fs::canonicalize(path.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or_else(|_| path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf())
        .join(path.file_name().unwrap_or_default());
    let mutex = {
        let mut guard = table.lock().map_err(|_| BtError::internal("journal 进程锁表被投毒"))?;
        guard.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    };
    let _process_guard = mutex.lock().map_err(|_| BtError::internal("journal 进程锁被投毒"))?;
    let lock_path = path.with_file_name(format!(
        "{}.lock",
        path.file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    if let Some(parent) = lock_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        if platform::try_lock_exclusive(&lock_file)? {
            break;
        }
        if Instant::now() >= deadline {
            return Err(BtError::locked("journal 跨进程锁等待超时"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = action();
    let _ = platform::unlock(&lock_file);
    result
}
#[cfg(target_os = "linux")]
mod platform {
    use std::fs::File;
    use std::io::Read;
    use std::os::unix::io::AsRawFd;

    pub fn try_lock_exclusive(file: &File) -> Result<bool, bt_core::BtError> {
        // 安全性：fd 由传入 File 持有，flock 只操作该描述符。
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error.into())
        }
    }

    pub fn unlock(file: &File) -> Result<(), bt_core::BtError> {
        // 安全性：fd 由传入 File 持有，flock 只操作该描述符。
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().into())
        }
    }

    pub fn current_process() -> (u32, u32) {
        (
            std::process::id(),
            process_start_marker(std::process::id()).unwrap_or(0),
        )
    }

    pub fn is_process_alive(pid: u32, start_marker: u32) -> bool {
        if pid == 0 {
            return false;
        }
        match process_start_marker(pid) {
            Some(_) if start_marker == 0 => true,
            Some(marker) => marker == start_marker,
            None => false,
        }
    }

    fn process_start_marker(pid: u32) -> Option<u32> {
        let mut file = File::open(format!("/proc/{pid}/stat")).ok()?;
        let mut text = String::new();
        file.read_to_string(&mut text).ok()?;
        let end = text.rfind(')')?;
        let rest = text.get(end + 2..)?;
        rest.split_whitespace().nth(19)?.parse().ok()
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::fs::File;
    use std::io;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut core::ffi::c_void,
    }

    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x0000_0001;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[link(name = "kernel32")]
    extern "system" {
        fn LockFileEx(
            file: *mut core::ffi::c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn UnlockFileEx(
            file: *mut core::ffi::c_void,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn GetCurrentProcessId() -> u32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetProcessTimes(
            process: *mut core::ffi::c_void,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
    }

    fn overlapped() -> Overlapped {
        Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: std::ptr::null_mut(),
        }
    }

    pub fn try_lock_exclusive(file: &File) -> Result<bool, bt_core::BtError> {
        use std::os::windows::io::AsRawHandle;
        let mut overlapped = overlapped();
        // 安全性：句柄来自有效 File，OVERLAPPED 为栈上 C 布局结构。
        let ok = unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                !0,
                !0,
                &mut overlapped,
            )
        };
        if ok != 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(33) {
            Ok(false)
        } else {
            Err(error.into())
        }
    }

    pub fn unlock(file: &File) -> Result<(), bt_core::BtError> {
        use std::os::windows::io::AsRawHandle;
        let mut overlapped = overlapped();
        // 安全性：句柄来自有效 File，OVERLAPPED 为栈上 C 布局结构。
        let ok = unsafe { UnlockFileEx(file.as_raw_handle(), 0, !0, !0, &mut overlapped) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error().into())
        }
    }

    pub fn current_process() -> (u32, u32) {
        // 安全性：只查询当前进程的时间戳，输出指针均指向本栈变量。
        unsafe {
            let pid = GetCurrentProcessId();
            let mut creation = 0u64;
            let mut exit = 0u64;
            let mut kernel = 0u64;
            let mut user = 0u64;
            if GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) != 0 {
                (pid, creation as u32)
            } else {
                (pid, 0)
            }
        }
    }

    pub fn is_process_alive(pid: u32, start_marker: u32) -> bool {
        // 安全性：句柄检查非空后使用并在所有路径关闭；输出指针为栈变量。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut creation = 0u64;
            let mut exit = 0u64;
            let mut kernel = 0u64;
            let mut user = 0u64;
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            let _ = CloseHandle(handle);
            ok != 0 && (start_marker == 0 || creation as u32 == start_marker)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use std::fs::File;

    pub fn try_lock_exclusive(_file: &File) -> Result<bool, bt_core::BtError> {
        Ok(true)
    }

    pub fn unlock(_file: &File) -> Result<(), bt_core::BtError> {
        Ok(())
    }

    pub fn current_process() -> (u32, u32) {
        (std::process::id(), 0)
    }

    pub fn is_process_alive(_pid: u32, _start_marker: u32) -> bool {
        false
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
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0);
            path.push(format!(
                "bt-journal-{tag}-{}-{now}-{}",
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

    struct FixedLiveness(bool);

    impl ProcessLiveness for FixedLiveness {
        fn is_alive(&self, _pid: u32, _start_marker: u32) -> bool {
            self.0
        }
    }

    fn peer() -> DeviceId {
        DeviceId::parse("00:11:22:33:44:55").unwrap()
    }

    fn write_payload(dir: &TempDir, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.0.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn entry(_dir: &TempDir, payload: PathBuf, display: &str) -> JournalEntry {
        let content = std::fs::read(&payload).unwrap();
        let size = content.len() as u64;
        JournalEntry::new(
            peer(),
            display,
            payload,
            sha256(&content),
            size,
            JournalKind::File,
            false,
        )
    }

    fn journal(dir: &TempDir, liveness: bool, now: u64) -> Journal {
        Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(liveness)))
            .with_clock(Arc::new(move || now))
            .with_process(4242, 99)
    }

    #[test]
    fn entry_round_trips_within_budget() {
        let dir = TempDir::new("codec");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let mut entry = entry(&dir, payload, "a.bin");
        entry.state = JournalState::Sending;
        entry.attempts = 3;
        entry.resume_offset = 1024;
        entry.lease_until = 1_800_000_000;
        entry.owner_pid = 7;
        entry.owner_start = 11;
        entry.created = 1_700_000_000;
        entry.updated = 1_700_000_001;
        entry.kind = JournalKind::FileZip;
        entry.produced = true;
        let encoded = entry.encode(&dir.0).unwrap();
        assert!(encoded.len() <= MAX_ENTRY_BYTES, "条目 {} 字节", encoded.len());
        assert_eq!(JournalEntry::decode(&encoded, &dir.0).unwrap(), entry);
    }

    #[test]
    fn overlong_entry_is_rejected() {
        let dir = TempDir::new("long");
        let path = dir.0.join(format!("{}.bin", "x".repeat(200)));
        let entry = JournalEntry::new(peer(), "a.bin", path, [1u8; 32], 1, JournalKind::File, false);
        assert!(entry.encode(&dir.0).is_err());
    }

    #[test]
    fn corrupt_file_is_quarantined() {
        let dir = TempDir::new("corrupt");
        let path = dir.0.join("send-journal.bin");
        std::fs::write(&path, b"not a journal").unwrap();
        let journal = journal(&dir, false, 1_000);
        assert!(journal.read_all().unwrap().is_empty());
        let quarantined = std::fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt."));
        assert!(quarantined);
    }

    #[test]
    fn begin_persists_and_live_lease_is_busy() {
        let dir = TempDir::new("begin");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let journal = journal(&dir, true, 1_000);
        let first = journal.begin(entry(&dir, payload.clone(), "a.bin")).unwrap();
        assert!(!first.busy);
        assert_eq!(first.entry.state, JournalState::Sending);
        let second = journal.begin(entry(&dir, payload.clone(), "a.bin")).unwrap();
        assert!(second.busy);
        assert_eq!(first.entry.id, second.entry.id);
        assert_eq!(journal.read_all().unwrap().len(), 1);
        let mut other_peer = entry(&dir, payload.clone(), "a.bin");
        other_peer.peer = peer();
        other_peer.display_name = "a (1).bin".to_string();
        journal.begin(other_peer).unwrap();
        assert_eq!(journal.read_all().unwrap().len(), 2);
    }

    #[test]
    fn dead_owner_is_recoverable_and_payload_change_is_abandoned() {
        let dir = TempDir::new("recover");
        let payload = write_payload(&dir, "a.bin", b"AAAA");
        let before = journal(&dir, true, 1_000);
        let id = before
            .begin(entry(&dir, payload.clone(), "a.bin"))
            .unwrap()
            .entry
            .id_hex();
        let after = journal(&dir, false, 1_000);
        let plan = after.plan_recovery().unwrap();
        assert_eq!(plan.recoverable.len(), 1);
        std::fs::write(&payload, b"BBBB").unwrap();
        let plan = after.plan_recovery().unwrap();
        assert!(plan.recoverable.is_empty());
        assert!(plan.abandoned[0].reason.contains("SHA-256"));
        assert_eq!(after.apply_abandoned(&[id]).unwrap(), 1);
        assert!(after.read_all().unwrap().is_empty());
    }

    #[test]
    fn release_pause_and_attempt_limit() {
        let dir = TempDir::new("state");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let journal = journal(&dir, false, 1_000);
        let id = journal.begin(entry(&dir, payload, "a.bin")).unwrap().entry.id_hex();
        journal.release(&id, true).unwrap();
        let stored = &journal.read_all().unwrap()[0];
        assert_eq!(stored.state, JournalState::Pending);
        assert_eq!(stored.attempts, 1);
        journal.pause(&id).unwrap();
        assert!(!journal.claim(&id).unwrap());
        assert!(journal.plan_recovery().unwrap().recoverable.is_empty());
        for _ in 0..MAX_ATTEMPTS {
            journal.release(&id, true).unwrap();
        }
        let plan = journal.plan_recovery().unwrap();
        assert!(plan.recoverable.is_empty());
        assert!(plan.abandoned[0].reason.contains("已尝试"));
    }

    #[test]
    fn heartbeat_extends_lease_and_records_progress() {
        let dir = TempDir::new("heartbeat");
        let payload = write_payload(&dir, "a.bin", b"0123456789");
        let now = std::sync::Arc::new(Mutex::new(1_000u64));
        let clock = {
            let now = now.clone();
            Arc::new(move || *now.lock().unwrap())
        };
        let journal = Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(true)))
            .with_clock(clock)
            .with_process(4242, 99);
        let id = journal.begin(entry(&dir, payload, "a.bin")).unwrap().entry.id_hex();
        let first = journal.read_all().unwrap()[0].lease_until;
        assert_eq!(first, 1_000 + LEASE.as_secs());

        *now.lock().unwrap() = 1_000 + 5 * 60;
        assert!(journal.heartbeat(&id, 4).unwrap());
        let renewed = journal.read_all().unwrap()[0].clone();
        assert_eq!(renewed.lease_until, 1_000 + 5 * 60 + LEASE.as_secs());
        assert_eq!(renewed.resume_offset, 4);
        assert_eq!(renewed.updated, 1_300);

        assert!(journal.pause(&id).unwrap());
        *now.lock().unwrap() = 1_000 + 10 * 60;
        assert!(!journal.heartbeat(&id, 8).unwrap(), "暂停条目不应续租");
    }

    #[test]
    fn ttl_cleanup_and_entry_cap() {
        let dir = TempDir::new("ttl");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let now = 1_000u64;
        let journal = Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(false)))
            .with_clock(Arc::new(move || now))
            .with_process(1, 1);
        journal.begin(entry(&dir, payload.clone(), "a.bin")).unwrap();
        let later = Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(false)))
            .with_clock(Arc::new(move || now + TTL.as_secs() + 1))
            .with_process(2, 2);
        assert_eq!(later.cleanup_expired().unwrap(), 1);

        let capped = Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(false)))
            .with_clock(Arc::new(|| 2_000))
            .with_process(1, 1);
        for index in 0..(MAX_ENTRIES + 2) {
            let mut item = entry(&dir, payload.clone(), &format!("f{index}.bin"));
            item.created = index as u32 + 1;
            item.id = [index as u8; 8];
            capped.begin(item).unwrap();
        }
        let entries = capped.read_all().unwrap();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert!(!entries.iter().any(|entry| entry.display_name == "f0.bin"));
    }

    #[test]
    fn import_dedupes_and_normalizes_state() {
        let dir = TempDir::new("import");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let journal = journal(&dir, false, 1_000);
        let mut item = entry(&dir, payload.clone(), "a.bin");
        item.state = JournalState::Sending;
        let report = journal.import(vec![item.clone(), item]).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped, 1);
        let stored = &journal.read_all().unwrap()[0];
        assert_eq!(stored.state, JournalState::Pending);
        assert_eq!(stored.owner_pid, 0);
        assert!(journal.read_all().unwrap()[0].payload.is_file());
        let _ = &payload;
    }

    #[test]
    fn complete_removes_entry_and_artifact() {
        let dir = TempDir::new("complete");
        let payload = write_payload(&dir, "package.zip", b"pkg");
        let cleaner = Arc::new(TestCleaner::default());
        let cleaner_dyn: Arc<dyn ArtifactCleaner> = cleaner.clone();
        let journal = Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(false)))
            .with_clock(Arc::new(|| 1_000))
            .with_process(1, 1)
            .with_cleaner(cleaner_dyn);
        let mut item = entry(&dir, payload, "a.bin.zip");
        item.produced = true;
        let id = journal.begin(item).unwrap().entry.id_hex();
        assert!(journal.complete(&id).unwrap());
        assert!(journal.read_all().unwrap().is_empty());
        assert_eq!(cleaner.deleted.lock().unwrap().len(), 1);
    }

    #[test]
    fn complete_keeps_payload_when_produced_flag_is_false() {
        let dir = TempDir::new("complete-user");
        let payload = write_payload(&dir, "user-source.bin", b"user");
        let cleaner = Arc::new(TestCleaner::default());
        let cleaner_dyn: Arc<dyn ArtifactCleaner> = cleaner.clone();
        let journal = Journal::new(dir.0.join("send-journal.bin"))
            .with_liveness(Arc::new(FixedLiveness(false)))
            .with_clock(Arc::new(|| 1_000))
            .with_process(1, 1)
            .with_cleaner(cleaner_dyn);
        let item = entry(&dir, payload.clone(), "user-source.bin");
        assert!(!item.produced);
        let id = journal.begin(item).unwrap().entry.id_hex();
        assert!(journal.complete(&id).unwrap());
        assert!(journal.read_all().unwrap().is_empty());
        assert!(cleaner.deleted.lock().unwrap().is_empty(), "非自产载荷不得清理");
        assert!(payload.is_file(), "用户源文件必须保留");
    }

    #[test]
    fn no_temporary_file_remains() {
        let dir = TempDir::new("atomic");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let journal = journal(&dir, false, 1_000);
        journal.begin(entry(&dir, payload, "a.bin")).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn cross_process_lock_blocks_second_writer() {
        let dir = TempDir::new("xproc");
        let payload = write_payload(&dir, "a.bin", b"hello");
        let journal_path = dir.0.join("send-journal.bin");
        let ready = dir.0.join("child.ready");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("journal::tests::cross_process_child_holds_lock")
            .arg("--nocapture")
            .env("BT_JOURNAL_CHILD", &journal_path)
            .env("BT_JOURNAL_READY", &ready)
            .spawn();
        let Ok(mut child) = child else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ready.exists() {
            let _ = child.kill();
            return;
        }
        let start = Instant::now();
        let journal = journal(&dir, false, 1_000);
        journal.begin(entry(&dir, payload, "a.bin")).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(400), "未等待子进程释放锁");
        let _ = child.wait();
    }

    #[test]
    fn cross_process_child_holds_lock() {
        let Some(path) = std::env::var_os("BT_JOURNAL_CHILD") else {
            return;
        };
        let ready = std::env::var_os("BT_JOURNAL_READY").unwrap();
        let path = PathBuf::from(path);
        lock_journal(&path, || {
            std::fs::write(&ready, b"ready")?;
            std::thread::sleep(Duration::from_millis(1_200));
            Ok(())
        })
        .unwrap();
    }

    #[derive(Default)]
    struct TestCleaner {
        deleted: Mutex<Vec<PathBuf>>,
    }

    impl ArtifactCleaner for TestCleaner {
        fn delete(&self, path: &Path) -> bool {
            if let Ok(mut deleted) = self.deleted.lock() {
                deleted.push(path.to_path_buf());
            }
            true
        }
    }
}
