use serde::Serialize;
use serde_json::{json, Value};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
    Events,
}

impl OutputMode {
    pub fn from_flags(json: bool, events: bool) -> Self {
        if events {
            Self::Events
        } else if json {
            Self::Json
        } else {
            Self::Human
        }
    }

    pub fn is_human(self) -> bool {
        self == Self::Human
    }
}

/// 输出单条 JSON 文档；`--events` 模式下以 `type=result` 作为最后一行。
pub fn emit_result<T: Serialize>(mode: OutputMode, value: &T) {
    match mode {
        OutputMode::Json => print_json_line(value),
        OutputMode::Events => {
            let mut body = serde_json::to_value(value).unwrap_or_else(|_| json!({ "ok": false }));
            if let Some(object) = body.as_object_mut() {
                object.insert("type".to_string(), json!("result"));
            }
            println!("{body}");
        }
        OutputMode::Human => {}
    }
}

pub fn emit_event(mode: OutputMode, value: Value) {
    if mode == OutputMode::Events {
        println!("{value}");
    }
}

pub fn print_json_line<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("错误：JSON 序列化失败：{error}"),
    }
}

/// 人类可读表格，按显示宽度对齐 CJK。
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    caps: Vec<Option<usize>>,
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|value| value.to_string()).collect(),
            rows: Vec::new(),
            caps: vec![None; headers.len()],
        }
    }

    pub fn cap(mut self, column: usize, width: usize) -> Self {
        if let Some(slot) = self.caps.get_mut(column) {
            *slot = Some(width);
        }
        self
    }

    pub fn row(&mut self, cells: impl IntoIterator<Item = String>) {
        let mut row: Vec<String> = cells.into_iter().collect();
        while row.len() < self.headers.len() {
            row.push(String::new());
        }
        row.truncate(self.headers.len());
        self.rows.push(row);
    }

    pub fn print(&self) {
        let mut widths: Vec<usize> = self.headers.iter().map(|value| display_width(value)).collect();
        for row in &self.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(display_width(cell));
            }
        }
        for (index, cap) in self.caps.iter().enumerate() {
            if let Some(cap) = cap {
                widths[index] = widths[index].min(*cap);
            }
        }
        let header: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .map(|(index, cell)| pad(cell, widths[index]))
            .collect();
        println!("{}", header.join("  "));
        let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
        println!("{}", rule.join("  "));
        for row in &self.rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(index, cell)| pad(&truncate(cell, widths[index]), widths[index]))
                .collect();
            println!("{}", cells.join("  "));
        }
    }
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn pad(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(padding))
}

fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    let limit = width.saturating_sub(1);
    for ch in text.chars() {
        let size = display_width(ch.encode_utf8(&mut [0u8; 4]));
        if used + size > limit {
            break;
        }
        out.push(ch);
        used += size;
    }
    out.push('…');
    out
}

/// 后端事件转 NDJSON 行；事件名与字段在 docs/CLI.md 冻结。
pub fn event_to_json(event: &bt_api::Event) -> Value {
    use bt_api::Event;
    match event {
        Event::Log { level, message, at } => json!({
            "type": "log",
            "level": level.as_str(),
            "message": message,
            "at": at,
        }),
        Event::ScanStarted => json!({ "type": "scan_started" }),
        Event::ScanFinished { count } => json!({ "type": "scan_finished", "count": count }),
        Event::DeviceList(devices) => json!({ "type": "device_list", "devices": devices }),
        Event::DeviceUpdated(device) => json!({ "type": "device_updated", "device": device }),
        Event::PairPrompt { request, device, kind } => json!({
            "type": "pair_prompt",
            "request": request,
            "device": device,
            "kind": kind,
        }),
        Event::PairResult { device_id, ok, error } => json!({
            "type": "pair_result",
            "device_id": device_id,
            "ok": ok,
            "error": error,
        }),
        Event::AssistantStateChanged { device_id, state } => json!({
            "type": "assistant_state_changed",
            "device_id": device_id,
            "state": state,
        }),
        Event::JobAdded { job } => json!({ "type": "job_added", "job": job }),
        Event::JobProgress {
            job_id,
            bytes,
            total,
            bps,
            eta_seconds,
        } => json!({
            "type": "job_progress",
            "job_id": job_id,
            "bytes": bytes,
            "total": total,
            "bps": bps,
            "eta_seconds": eta_seconds,
        }),
        Event::JobStateChanged { job } => json!({ "type": "job_state_changed", "job": job }),
        Event::QueueIdle => json!({ "type": "queue_idle" }),
        Event::ReceiveListeningChanged { listening } => {
            json!({ "type": "receive_listening", "listening": listening })
        }
        Event::ReceiveProgress { bytes, total, name } => json!({
            "type": "receive_progress",
            "bytes": bytes,
            "total": total,
            "name": name,
        }),
        Event::ReceiveAsk { id, name, size } => json!({
            "type": "receive_ask",
            "id": id,
            "name": name,
            "size": size,
        }),
        Event::ReceiveDone { ok, path, error } => json!({
            "type": "receive_done",
            "ok": ok,
            "path": path,
            "error": error,
        }),
        Event::ConfigChanged { config } => json!({ "type": "config_changed", "config": config }),
        Event::RecordsChanged => json!({ "type": "records_changed" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_width_counts_two_columns() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("中文"), 4);
        assert_eq!(display_width("a中b"), 4);
    }

    #[test]
    fn truncate_respects_display_width() {
        let text = truncate("abcdef", 4);
        assert_eq!(display_width(&text), 4);
        assert!(text.ends_with('…'));
        assert_eq!(truncate("abc", 4), "abc");
    }
}
