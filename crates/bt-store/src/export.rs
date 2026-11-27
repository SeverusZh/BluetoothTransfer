use std::path::Path;

use bt_api::{ExportFormat, ExportSummary, RecordQuery};
use bt_core::{BtError, Record};
use serde::Serialize;

use crate::Store;

/// 按查询条件导出记录；JSONL 走紧凑字段集，单条控制在 250 字节内。
pub fn export_records(
    store: &Store,
    path: &Path,
    query: &RecordQuery,
    format: ExportFormat,
) -> Result<ExportSummary, BtError> {
    let records = store.query_records(query)?;
    let rendered = match format {
        ExportFormat::Jsonl => render_jsonl(&records)?,
        ExportFormat::Csv => render_csv(&records),
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, rendered.as_bytes())?;
    Ok(ExportSummary {
        path: path.to_path_buf(),
        rows: records.len() as u64,
        bytes: rendered.len() as u64,
        format,
    })
}

/// 紧凑导出视图，省略空字段以压缩单条体积。
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

pub fn render_jsonl(records: &[Record]) -> Result<String, BtError> {
    let mut out = String::with_capacity(records.len() * 200);
    for record in records {
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
        let line = serde_json::to_string(&row).map_err(|error| BtError::storage(format!("导出序列化失败：{error}")))?;
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

pub fn render_csv(records: &[Record]) -> String {
    let mut out = String::from("id,direction,channel,kind,status,device,name,bytes,started_at,finished_at\n");
    for record in records {
        out.push_str(&format!(
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
        ));
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{Channel, DeviceId, Direction, RecordKind, RecordStatus};

    fn sample() -> Record {
        let mut record = Record::running(
            Direction::Send,
            Channel::Opp,
            RecordKind::File,
            "photo.jpg",
            "2026-01-02T03:04:05Z",
        );
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
        let text = render_jsonl(&[sample()]).unwrap();
        let line = text.lines().next().unwrap();
        assert!(line.len() <= 250, "单条 JSONL 超出预算：{}", line.len());
        assert!(line.starts_with('{'));
        assert!(line.contains("\"name\":\"photo.jpg\""));
    }

    #[test]
    fn csv_header_and_row_are_rendered() {
        let text = render_csv(&[sample()]);
        assert!(text.starts_with("id,direction,channel,kind,status,device,name,bytes,started_at,finished_at"));
        assert!(text.contains("photo.jpg"));
        assert_eq!(text.lines().count(), 2);
    }
}
