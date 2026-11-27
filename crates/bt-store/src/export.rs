use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use bt_api::{ExportFormat, ExportSummary, RecordQuery};
use bt_core::{BtError, Record};
use serde::Serialize;

use crate::Store;

/// 按查询条件流式导出记录；不把结果集整体载入内存，也没有行数截断。
pub fn export_records(
    store: &Store,
    path: &Path,
    query: &RecordQuery,
    format: ExportFormat,
) -> Result<ExportSummary, BtError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = File::create(path)?;
    let mut writer = CountingWriter {
        inner: BufWriter::new(file),
        bytes: 0,
    };
    let mut first = true;
    let rows = store.stream_records(query, |record| {
        match format {
            ExportFormat::Csv => {
                if first {
                    writer.write_all(CSV_HEADER.as_bytes())?;
                    first = false;
                }
                writer.write_all(render_csv_row(record)?.as_bytes())?;
            }
            ExportFormat::Jsonl => {
                writer.write_all(render_jsonl_row(record)?.as_bytes())?;
            }
        }
        Ok(())
    })?;
    writer.flush()?;
    Ok(ExportSummary {
        path: path.to_path_buf(),
        rows,
        bytes: writer.bytes,
        format,
    })
}

pub const CSV_HEADER: &str = "id,direction,channel,kind,status,device,name,bytes,started_at,finished_at\n";

#[derive(Debug, Serialize)]
struct ExportRow<'a> {
    id: i64,
    dir: &'a str,
    ch: &'a str,
    kind: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<&'a str>,
    name: &'a str,
    bytes: u64,
    at: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    done_at: Option<&'a str>,
}

pub fn render_jsonl_row(record: &Record) -> Result<String, BtError> {
    let row = ExportRow {
        id: record.id,
        dir: record.direction.as_str(),
        ch: record.channel.as_str(),
        kind: record.kind.as_str(),
        status: record.status.as_str(),
        device: Some(record.device_name.as_str()).filter(|value| !value.is_empty()),
        name: record.display_name.as_str(),
        bytes: record.byte_size,
        at: record.started_at.as_str(),
        done_at: record.finished_at.as_deref(),
    };
    let mut line = serde_json::to_string(&row).map_err(|error| BtError::storage(format!("导出序列化失败：{error}")))?;
    line.push('\n');
    Ok(line)
}

pub fn render_jsonl(records: &[Record]) -> Result<String, BtError> {
    let mut out = String::with_capacity(records.len() * 200);
    for record in records {
        out.push_str(&render_jsonl_row(record)?);
    }
    Ok(out)
}

pub fn render_csv_row(record: &Record) -> Result<String, BtError> {
    Ok(format!(
        "{},{},{},{},{},{},{},{},{},{}\n",
        record.id,
        record.direction.as_str(),
        record.channel.as_str(),
        record.kind.as_str(),
        record.status.as_str(),
        csv_field(&record.device_name),
        csv_field(&record.display_name),
        record.byte_size,
        record.started_at,
        record.finished_at.as_deref().unwrap_or_default()
    ))
}

pub fn render_csv(records: &[Record]) -> String {
    let mut out = String::from(CSV_HEADER);
    for record in records {
        if let Ok(row) = render_csv_row(record) {
            out.push_str(&row);
        }
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let guard = value
        .chars()
        .next()
        .is_some_and(|first| matches!(first, '=' | '+' | '-' | '@' | '\t' | '\r'));
    let quoted = value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r');
    let escaped = value.replace('"', "\"\"");
    match (guard, quoted) {
        (true, _) => format!("\"'{escaped}\""),
        (false, true) => format!("\"{escaped}\""),
        (false, false) => value.to_string(),
    }
}

struct CountingWriter {
    inner: BufWriter<File>,
    bytes: u64,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{Channel, DeviceId, Direction, RecordKind, RecordStatus};
    use std::path::PathBuf;

    fn sample(name: &str) -> Record {
        let mut record = Record::running(
            Direction::Send,
            Channel::Opp,
            RecordKind::File,
            name,
            "2026-01-02T03:04:05Z",
        );
        record.id = 7;
        record.device_id = Some(DeviceId::parse("AA:BB:CC:DD:EE:05").unwrap());
        record.device_name = "Pixel".to_string();
        record.byte_size = 1024;
        record.transferred = 1024;
        record.status = RecordStatus::Completed;
        record.finished_at = Some("2026-01-02T03:04:06Z".to_string());
        record
    }

    #[test]
    fn jsonl_line_stays_within_budget() {
        let text = render_jsonl(&[sample("photo.jpg")]).unwrap();
        let line = text.lines().next().unwrap();
        assert!(line.len() <= 250, "单条 JSONL 超出预算：{}", line.len());
        assert!(line.starts_with('{'));
        assert!(line.contains("\"name\":\"photo.jpg\""));
        assert!(line.contains("\"done_at\""));
    }

    #[test]
    fn export_has_no_hundred_thousand_row_cap() {
        let dir = std::env::temp_dir().join(format!(
            "bt-export-100k-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open_in_memory().unwrap();
        let records: Vec<Record> = (0..100_000i64)
            .map(|index| {
                let mut record = sample(&format!("file-{index}.bin"));
                record.id = index + 1;
                record
            })
            .collect();
        store.insert_records(&records).unwrap();
        let path = dir.join("all.jsonl");
        let summary = export_records(&store, &path, &RecordQuery::default(), ExportFormat::Jsonl).unwrap();
        assert_eq!(summary.rows, 100_000);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 100_000);
        eprintln!("MEASURE export_100k rows={} bytes={}", summary.rows, summary.bytes);
        let sample_line = render_jsonl(&[sample("photo.jpg")]).unwrap();
        eprintln!(
            "MEASURE export_jsonl_line={} csv_line={}",
            sample_line.lines().next().unwrap().len(),
            render_csv_row(&sample("photo.jpg")).unwrap().len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_matches_frozen_header_and_escapes() {
        let mut record = sample("a,b\"c.txt");
        record.device_name = "=cmd".to_string();
        let text = render_csv(&[record]);
        assert!(text.starts_with("id,direction,channel,kind,status,device,name,bytes,started_at,finished_at"));
        assert!(text.contains("\"'=cmd\""));
        assert!(text.contains("\"a,b\"\"c.txt\""));
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn export_streams_every_row_without_cap() {
        let dir = std::env::temp_dir().join(format!(
            "bt-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open_in_memory().unwrap();
        for index in 0..1200 {
            let mut record = sample(&format!("f{index}.bin"));
            record.id = index as i64 + 1;
            store.insert_record(&record).unwrap();
        }
        let csv_path = dir.join("all.csv");
        let summary = export_records(&store, &csv_path, &RecordQuery::default(), ExportFormat::Csv).unwrap();
        assert_eq!(summary.rows, 1200);
        assert_eq!(std::fs::read_to_string(&csv_path).unwrap().lines().count(), 1201);
        let jsonl_path = dir.join("all.jsonl");
        let summary = export_records(&store, &jsonl_path, &RecordQuery::default(), ExportFormat::Jsonl).unwrap();
        assert_eq!(summary.rows, 1200);
        assert_eq!(std::fs::read_to_string(&jsonl_path).unwrap().lines().count(), 1200);
        let limited = export_records(
            &store,
            &dir.join("limited.csv"),
            &RecordQuery::latest(5),
            ExportFormat::Csv,
        )
        .unwrap();
        assert_eq!(limited.rows, 5);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = PathBuf::new();
    }
}
