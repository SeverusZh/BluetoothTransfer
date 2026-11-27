use std::path::{Path, PathBuf};

use bt_core::{sha256, DeviceId};
use bt_engine::{JournalEntry, JournalKind, JournalState, ResumeSidecar, MAX_ENTRY_BYTES, SIDECAR_LIMIT};

fn base_dir() -> PathBuf {
    PathBuf::from("/home/user/.local/share/BluetoothTransfer")
}

#[test]
fn result_file_budgets_are_measured_and_enforced() {
    let base = base_dir();
    let sidecar = ResumeSidecar::new(sha256(b"content"), 262_144, 4 * 1024 * 1024).encode();
    let mut entry = JournalEntry::new(
        DeviceId::parse("AA:BB:CC:DD:EE:FF").unwrap(),
        "季度报告-2026-最终版.txt".to_string(),
        base.join("pending/bt_asst_pending_0123456789abcdef0123456789abcdef.zip"),
        sha256(b"payload"),
        12_345_678,
        JournalKind::FileZip,
        true,
    );
    entry.state = JournalState::Sending;
    entry.attempts = 2;
    entry.resume_offset = 4_194_304;
    entry.lease_until = 1_800_000_000;
    entry.owner_pid = 4242;
    entry.owner_start = 99;
    entry.created = 1_700_000_000;
    entry.updated = 1_700_000_100;
    let encoded = entry.encode(&base).unwrap();
    let assistant_chunk = bt_engine::SendSettings::default().assistant_chunk_size;
    let assistant_overhead = (bt_engine::HEADER_LEN + bt_engine::DATA_OVERHEAD) as f64 / assistant_chunk as f64 * 100.0;
    let opp_chunk = 65_023f64;
    let opp_overhead = 6.0 / opp_chunk * 100.0;
    eprintln!(
        "MEASURE sidecar={} journal_entry={} journal_limit={} sidecar_limit={} assistant_chunk={} assistant_overhead_pct={:.5} opp_overhead_pct={:.5}",
        sidecar.len(),
        encoded.len(),
        MAX_ENTRY_BYTES,
        SIDECAR_LIMIT,
        assistant_chunk,
        assistant_overhead,
        opp_overhead
    );
    assert_eq!(sidecar.len(), 53);
    assert!(sidecar.len() <= SIDECAR_LIMIT);
    assert!(encoded.len() <= MAX_ENTRY_BYTES);

    let mut worst = JournalEntry::new(
        DeviceId::parse("AA:BB:CC:DD:EE:FF").unwrap(),
        "x".repeat(110),
        PathBuf::from("bt_asst_pending_0123456789abcdef0123456789abcdef.zip"),
        sha256(b"payload"),
        1,
        JournalKind::FileZip,
        true,
    );
    worst.state = JournalState::Pending;
    let worst_bytes = worst
        .encode(Path::new("/home/user/.local/share/BluetoothTransfer/pending"))
        .unwrap();
    eprintln!("MEASURE journal_entry_worst={}", worst_bytes.len());
    assert!(worst_bytes.len() <= MAX_ENTRY_BYTES);
    assert_eq!(worst_bytes.len(), MAX_ENTRY_BYTES.saturating_sub(3));
    assert!(assistant_overhead <= 0.01);
    assert!(opp_overhead <= 0.03);
}
