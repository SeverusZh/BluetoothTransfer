use std::path::{Path, PathBuf};
use std::process::Output;

use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

const PHONE: &str = "AA:BB:CC:00:00:01";
const LAPTOP: &str = "AA:BB:CC:00:00:02";

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("创建临时目录"),
        }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.home().join(relative)
    }
}

fn run(env: &Env, program: &str, args: &[&str]) -> Output {
    run_with_stdin(env, program, args, None)
}

fn run_with_stdin(env: &Env, program: &str, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = assert_cmd::Command::cargo_bin(program).expect("二进制存在");
    command
        .env("BT_HOME", env.home())
        .env("BT_BACKEND", "mock")
        .env_remove("APPDATA")
        .env_remove("LOCALAPPDATA")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .args(args);
    if let Some(input) = stdin {
        command.write_stdin(input);
    }
    command.output().expect("命令可执行")
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout 不是单个 JSON：{error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn ndjson(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("NDJSON 行合法"))
        .collect()
}

fn seed_journal(env: &Env, name: &str, content: &[u8], paused: bool) -> (String, PathBuf) {
    let paths = bt_app::AppPaths::portable(env.home());
    paths.create_dirs().expect("创建便携目录");
    let payload = paths.pending_dir.join(format!("{name}.bin"));
    std::fs::write(&payload, content).expect("写入续传载荷");
    let peer = bt_core::DeviceId::parse(PHONE).expect("设备地址");
    let entry = bt_engine::JournalEntry::new(
        peer,
        name,
        payload.clone(),
        bt_core::sha256(content),
        content.len() as u64,
        bt_engine::JournalKind::File,
        false,
    );
    let journal = bt_engine::Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
    let begun = journal.begin(entry).expect("写入 journal");
    let id = begun.entry.id_hex();
    if paused {
        journal.pause(&id).expect("暂停 journal 条目");
    } else {
        journal.release(&id, false).expect("释放 journal 租约");
    }
    (id, payload)
}

fn repl(env: &Env, lines: &[&str]) -> Vec<Value> {
    let mut input = lines.join("\n");
    input.push_str("\nquit\n");
    let output = run_with_stdin(env, "bt", &["repl", "--json"], Some(&input));
    assert_eq!(
        code(&output),
        0,
        "repl 退出码：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    ndjson(&output)
}

#[test]
fn help_and_version_are_available() {
    let env = Env::new();
    let help = run(&env, "bt", &["--help"]);
    assert_eq!(code(&help), 0);
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("devices"));
    assert!(stdout.contains("send"));
    assert!(stdout.contains("recv"));
    assert!(stdout.contains("records"));
    assert!(stdout.contains("config"));
    assert!(stdout.contains("selftest"));
    assert!(stdout.contains("--json"));

    let version = run(&env, "bt", &["--version"]);
    assert_eq!(code(&version), 0);
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("bt {}", env!("CARGO_PKG_VERSION"))
    );

    let sub_help = run(&env, "bt", &["send", "file", "--help"]);
    assert_eq!(code(&sub_help), 0);
    assert!(predicate::str::contains("--zip").eval(&String::from_utf8_lossy(&sub_help.stdout)));
}

#[test]
fn usage_errors_exit_2() {
    let env = Env::new();
    assert_eq!(code(&run(&env, "bt", &["unknown-command"])), 2);
    assert_eq!(code(&run(&env, "bt", &["send"])), 2);
    assert_eq!(code(&run(&env, "bt", &["send", "file"])), 2);
    assert_eq!(code(&run(&env, "bt", &["devices", "pair"])), 2);
    assert_eq!(code(&run(&env, "bt", &["recv", "--ask", "--no-ask"])), 2);
    assert_eq!(
        code(&run(&env, "bt", &["send", "text", PHONE, "hi", "--mode", "nope"])),
        2
    );
    assert_eq!(code(&run(&env, "bt", &["export", "xml", "out.xml"])), 2);
    assert_eq!(code(&run(&env, "bt", &["records", "list", "--peer", "bad"])), 2);
    assert_eq!(code(&run(&env, "bt", &["records", "clear"])), 2);
    assert_eq!(code(&run(&env, "bt", &["config", "set", "mode"])), 2);
    assert_eq!(code(&run(&env, "bt", &["devices", "pair", "bad-address"])), 2);
}

#[test]
fn devices_list_and_sort() {
    let env = Env::new();
    let output = run(&env, "bt", &["devices", "list", "--json"]);
    assert_eq!(code(&output), 0);
    let value = json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["count"], 2);
    let devices = value["devices"].as_array().unwrap();
    assert_eq!(devices[0]["id"], PHONE);
    assert_eq!(devices[0]["name"], "Mock Phone");
    assert_eq!(devices[1]["id"], LAPTOP);
    assert_eq!(devices[0]["assistant_state"], "available");
    assert_eq!(devices[0]["channels"][0], "opp");
    assert_eq!(devices[0]["channels"][1], "assistant");

    let by_name = run(&env, "bt", &["devices", "list", "--sort", "name", "--json"]);
    let value = json(&by_name);
    assert_eq!(value["devices"][0]["name"], "Mock Phone");

    let by_favorite = run(&env, "bt", &["devices", "list", "--sort", "favorite", "--json"]);
    let value = json(&by_favorite);
    assert_eq!(value["devices"][0]["id"], PHONE);
}

#[test]
fn devices_scan_reports_devices_and_events() {
    let env = Env::new();
    let output = run(&env, "bt", &["devices", "scan", "--seconds", "1", "--json"]);
    assert_eq!(code(&output), 0);
    let value = json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["count"], 2);
    assert_eq!(value["duration_s"], 1);

    let paired = run(
        &env,
        "bt",
        &["devices", "scan", "--seconds", "1", "--paired-only", "--json"],
    );
    let value = json(&paired);
    let all = json(&run(&env, "bt", &["devices", "list", "--json"]));
    let paired_count = all["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|device| device["paired"] == true)
        .count();
    assert_eq!(value["count"].as_u64().unwrap(), paired_count as u64);

    let events = run(&env, "bt", &["devices", "scan", "--seconds", "1", "--events"]);
    assert_eq!(code(&events), 0);
    let lines = ndjson(&events);
    let types: Vec<&str> = lines.iter().filter_map(|line| line["type"].as_str()).collect();
    assert!(types.contains(&"scan_started"));
    assert!(types.contains(&"device_list"));
    assert!(types.contains(&"scan_finished"));
    assert_eq!(types.last(), Some(&"result"));
}

#[test]
fn devices_alias_favorite_and_pair_work_in_one_session() {
    let env = Env::new();
    let outputs = repl(
        &env,
        &[
            &format!("devices alias {PHONE} 工作机 --json"),
            "devices list --json",
            &format!("devices favorite {LAPTOP} --json"),
            &format!("devices favorite {LAPTOP} --unset --json"),
            &format!("devices pair {LAPTOP} --json"),
        ],
    );
    assert_eq!(outputs.len(), 5);
    assert_eq!(outputs[0]["alias"], "工作机");
    let listed = outputs[1]["devices"].as_array().unwrap();
    let phone = listed.iter().find(|device| device["id"] == PHONE).unwrap();
    assert_eq!(phone["alias"], "工作机");
    assert_eq!(outputs[2]["favorite"], true);
    assert_eq!(outputs[3]["favorite"], false);
    assert_eq!(outputs[4]["ok"], true);
    assert_eq!(outputs[4]["paired"], true);

    let cleared = run(&env, "bt", &["devices", "alias", PHONE, "--json"]);
    assert_eq!(code(&cleared), 0);
    assert_eq!(json(&cleared)["alias"], Value::Null);
}

#[test]
fn devices_detect_uses_three_state_exit_codes() {
    let env = Env::new();
    let present = run(&env, "bt", &["devices", "detect", PHONE, "--json"]);
    assert_eq!(code(&present), 0);
    assert_eq!(json(&present)["state"], "present");

    let unknown = run(&env, "bt", &["devices", "detect", "AA:BB:CC:00:99:99", "--json"]);
    assert_eq!(code(&unknown), 5);
    let value = json(&unknown);
    assert_eq!(value["state"], "unknown");
    assert!(value["error"].as_str().unwrap().contains("设备不存在"));
}

#[test]
fn send_text_returns_jobs_and_summary() {
    let env = Env::new();
    let output = run(&env, "bt", &["send", "text", PHONE, "hello", "world", "--json"]);
    assert_eq!(code(&output), 0);
    let value = json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["summary"]["completed"], 1);
    assert_eq!(value["jobs"][0]["state"], "completed");
    assert_eq!(value["jobs"][0]["item"]["source"]["value"], "hello world");
    assert_eq!(value["jobs"][0]["channel"], "opp");

    let assistant = run(
        &env,
        "bt",
        &["send", "text", PHONE, "hi", "--mode", "assistant", "--json"],
    );
    assert_eq!(code(&assistant), 0);
    assert_eq!(json(&assistant)["jobs"][0]["channel"], "assistant");

    let named = run(
        &env,
        "bt",
        &["send", "text", PHONE, "note", "--name", "自定义.txt", "--json"],
    );
    assert_eq!(code(&named), 0);
    assert_eq!(json(&named)["jobs"][0]["item"]["display_name"], "自定义.txt");
}

#[test]
fn send_text_name_is_used_by_the_record() {
    let env = Env::new();
    let outputs = repl(
        &env,
        &[
            "send text AA:BB:CC:00:00:01 hello --name 报告名称 --json",
            "records list --json",
        ],
    );
    assert_eq!(outputs[0]["jobs"][0]["item"]["display_name"], "报告名称");
    assert_eq!(outputs[1]["records"][0]["display_name"], "报告名称");
}

#[test]
fn send_events_are_ndjson_with_result_line() {
    let env = Env::new();
    let output = run(&env, "bt", &["send", "text", PHONE, "hi", "--events"]);
    assert_eq!(code(&output), 0);
    let lines = ndjson(&output);
    let types: Vec<&str> = lines.iter().filter_map(|line| line["type"].as_str()).collect();
    assert!(types.contains(&"job_added"));
    assert!(types.contains(&"job_progress"));
    assert!(types.contains(&"job_state_changed"));
    assert_eq!(types.last(), Some(&"result"));
    assert_eq!(lines.last().unwrap()["ok"], true);
}

#[test]
fn send_file_and_folder_and_zip() {
    let env = Env::new();
    let first = env.path("a.txt");
    let second = env.path("b.txt");
    std::fs::write(&first, b"alpha").unwrap();
    std::fs::write(&second, b"beta").unwrap();
    let folder = env.path("payload");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("x.txt"), b"x").unwrap();

    let files = run(
        &env,
        "bt",
        &[
            "send",
            "files",
            PHONE,
            first.to_str().unwrap(),
            second.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code(&files), 0);
    let value = json(&files);
    assert_eq!(value["summary"]["total"], 2);
    assert_eq!(value["summary"]["completed"], 2);

    let zipped = run(
        &env,
        "bt",
        &["send", "file", PHONE, first.to_str().unwrap(), "--zip", "--json"],
    );
    assert_eq!(code(&zipped), 0);
    let value = json(&zipped);
    assert_eq!(value["jobs"][0]["item"]["display_name"], "a.txt.zip");
    let archive = value["jobs"][0]["item"]["source"]["value"].as_str().unwrap();
    assert!(archive.starts_with(env.home().to_str().unwrap()));
    assert!(archive.contains("pending"));
    assert!(!Path::new(archive).exists(), "临时 zip 未清理");

    let folder_output = run(
        &env,
        "bt",
        &["send", "folder", PHONE, folder.to_str().unwrap(), "--json"],
    );
    assert_eq!(code(&folder_output), 0);
    assert_eq!(json(&folder_output)["jobs"][0]["item"]["kind"], "folder");
}

#[test]
fn send_errors_have_stable_codes_and_shapes() {
    let env = Env::new();
    let missing = run(&env, "bt", &["send", "file", PHONE, "/nonexistent/bt-file", "--json"]);
    assert_eq!(code(&missing), 2);
    let value = json(&missing);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_argument");

    let target = run(&env, "bt", &["send", "text", "AA:BB:CC:00:99:99", "hi", "--json"]);
    assert_eq!(code(&target), 5);
    let value = json(&target);
    assert_eq!(value["error"]["kind"], "not_found");
}

#[test]
fn records_export_and_stats_flow_in_one_session() {
    let env = Env::new();
    let export_path = env.path("records.csv");
    let outputs = repl(
        &env,
        &[
            &format!("send text {PHONE} hello --json"),
            "records list --json",
            "records stats --json",
            &format!("export csv {} --json", export_path.display()),
            "records delete 1 --json",
            "records list --json",
            "records clear --yes --json",
            "records stats --json",
        ],
    );
    assert_eq!(outputs.len(), 8);
    assert_eq!(outputs[1]["count"], 1);
    assert_eq!(outputs[1]["records"][0]["display_name"], "hello");
    assert_eq!(outputs[2]["stats"]["total"], 1);
    assert_eq!(outputs[3]["rows"], 1);
    assert_eq!(outputs[4]["deleted"], true);
    assert_eq!(outputs[5]["count"], 0);
    assert_eq!(outputs[7]["stats"]["total"], 0);
    let csv = std::fs::read_to_string(&export_path).unwrap();
    assert!(csv.starts_with("id,direction,channel,kind,status,device,name,bytes,started_at"));
    assert!(csv.contains("hello"));
}

#[test]
fn records_filters_and_missing_delete() {
    let env = Env::new();
    let output = run(
        &env,
        "bt",
        &["records", "list", "--direction", "recv", "--type", "file", "--json"],
    );
    assert_eq!(code(&output), 0);
    assert_eq!(json(&output)["count"], 0);

    let missing = run(&env, "bt", &["records", "delete", "42", "--json"]);
    assert_eq!(code(&missing), 5);
    assert_eq!(json(&missing)["error"]["kind"], "not_found");
}

#[test]
fn config_flow_reports_effective_values() {
    let env = Env::new();
    let outputs = repl(
        &env,
        &[
            "config list --json",
            "config set mode opp --json",
            "config get mode --json",
            "config set opp_chunk_size 999999 --json",
            "config set receive_ask false --json",
        ],
    );
    let config = &outputs[0]["config"];
    assert_eq!(config.as_object().unwrap().len(), 12);
    assert_eq!(config["opp_chunk_size"], 65023);
    assert_eq!(outputs[1]["value"], "opp");
    assert_eq!(outputs[2]["value"], "opp");
    assert_eq!(outputs[3]["value"], 65023);
    assert_eq!(outputs[4]["value"], false);
}

#[test]
fn password_never_appears_in_argv_like_output_or_json() {
    let env = Env::new();
    let secret = "s3cret-value-123";
    let output = run_with_stdin(&env, "bt", &["config", "set", "auth-password", "--json"], Some(secret));
    assert_eq!(code(&output), 0);
    assert_eq!(json(&output)["set"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));

    let cleared = run_with_stdin(&env, "bt", &["config", "set", "auth-password", "--json"], Some("\n"));
    assert_eq!(code(&cleared), 0);
    assert_eq!(json(&cleared)["set"], false);

    let get = run(&env, "bt", &["config", "get", "auth-password", "--json"]);
    assert_eq!(code(&get), 2);
    assert!(!String::from_utf8_lossy(&get.stdout).contains(secret));
}

#[test]
fn selftest_reports_checks() {
    let env = Env::new();
    let output = run(&env, "bt", &["selftest", "--json"]);
    assert_eq!(code(&output), 0);
    let value = json(&output);
    assert_eq!(value["ok"], true);
    assert!(!value["checks"].as_array().unwrap().is_empty());
}

#[test]
fn migrate_reports_warnings_for_missing_legacy_root() {
    let env = Env::new();
    let legacy = env.path("legacy");
    let output = run(&env, "bt", &["migrate", "--from", legacy.to_str().unwrap(), "--json"]);
    assert_eq!(code(&output), 0);
    let value = json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(value["dry_run"], false);
}

#[test]
fn resume_empty_state_is_stable() {
    let env = Env::new();
    let list = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(code(&list), 0);
    let value = json(&list);
    assert_eq!(value["ok"], true);
    assert_eq!(value["count"], 0);
    assert_eq!(value["entries"].as_array().unwrap().len(), 0);
    assert!(value["journal_dir"].as_str().unwrap().ends_with("journal"));

    let cancel = run(&env, "bt", &["resume", "cancel", "--json"]);
    assert_eq!(code(&cancel), 0);
    assert_eq!(json(&cancel)["canceled"], 0);

    let run_output = run(&env, "bt", &["resume", "run", "--json"]);
    assert_eq!(code(&run_output), 0);
    assert_eq!(json(&run_output)["resumed"], 0);
}

#[test]
fn resume_list_run_and_cancel_use_the_real_journal() {
    let env = Env::new();
    let (id, _payload) = seed_journal(&env, "续传示例.bin", b"resume-payload", false);
    let listed = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(code(&listed), 0);
    let value = json(&listed);
    assert_eq!(value["count"], 1);
    assert_eq!(value["entries"][0]["id"], id);
    assert_eq!(value["entries"][0]["display_name"], "续传示例.bin");
    assert_eq!(value["entries"][0]["state"], "pending");

    let resumed = run(&env, "bt", &["resume", "run", "--json"]);
    assert_eq!(code(&resumed), 0);
    let value = json(&resumed);
    assert_eq!(value["resumed"], 1);
    assert_eq!(value["summary"]["completed"], 1);
    assert_eq!(value["jobs"][0]["channel"], "assistant");
    assert_eq!(value["jobs"][0]["item"]["display_name"], "续传示例.bin");

    let after = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(json(&after)["count"], 0, "完成后应清理 journal 条目");
}

#[test]
fn resume_run_recovers_paused_entries_and_prunes_missing_payloads() {
    let env = Env::new();
    let (_paused_id, _payload) = seed_journal(&env, "暂停.bin", b"paused-payload", true);
    let resumed = run(&env, "bt", &["resume", "run", "--json"]);
    assert_eq!(code(&resumed), 0);
    assert_eq!(json(&resumed)["summary"]["completed"], 1);

    let (missing_id, payload) = seed_journal(&env, "缺失.bin", b"missing-payload", false);
    std::fs::remove_file(payload).expect("删除载荷");
    let failed = run(&env, "bt", &["resume", "run", "--json"]);
    assert_eq!(code(&failed), 1);
    let value = json(&failed);
    assert_eq!(value["resumed"], 0);
    let warnings = value["warnings"].as_array().unwrap();
    assert!(warnings
        .iter()
        .any(|warning| warning.as_str().unwrap().contains(&missing_id)));
    let after = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(json(&after)["count"], 0, "缺失载荷的条目应被清除");
}

#[test]
fn resume_cancel_removes_entries() {
    let env = Env::new();
    let (id, _payload) = seed_journal(&env, "取消.bin", b"cancel-payload", false);
    let canceled = run(&env, "bt", &["resume", "cancel", "--json"]);
    assert_eq!(code(&canceled), 0);
    assert_eq!(json(&canceled)["canceled"], 1);
    let after = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(json(&after)["count"], 0);
    let _ = id;
}

#[test]
fn hidden_compat_aliases_work() {
    let env = Env::new();
    let scan = run(&env, "bt", &["opp-scan", "--seconds", "1", "--json"]);
    assert_eq!(code(&scan), 0);
    assert_eq!(json(&scan)["count"], 2);

    let detect = run(&env, "bt", &["detect", PHONE, "--json"]);
    assert_eq!(code(&detect), 0);
    assert_eq!(json(&detect)["state"], "present");

    let stats = run(&env, "bt", &["stats", "--json"]);
    assert_eq!(code(&stats), 0);
    assert_eq!(json(&stats)["stats"]["total"], 0);

    let text = run(&env, "bt", &["opp-send-text", PHONE, "legacy", "--json"]);
    assert_eq!(code(&text), 0);
    assert_eq!(json(&text)["summary"]["completed"], 1);
}

#[test]
fn btrecv_help_and_version() {
    let env = Env::new();
    let help = run(&env, "btrecv", &["--help"]);
    assert_eq!(code(&help), 0);
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("btrecv"));
    assert!(stdout.contains("--dir"));
    assert!(stdout.contains("--no-ask"));

    let version = run(&env, "btrecv", &["--version"]);
    assert_eq!(code(&version), 0);
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("btrecv "));
}

#[cfg(unix)]
fn sigint_script(program: &str, extra: &str) -> String {
    let binary = match program {
        "btrecv" => env!("CARGO_BIN_EXE_btrecv"),
        _ => env!("CARGO_BIN_EXE_bt"),
    };
    format!("\"{binary}\" {extra} & pid=$!; sleep 1; kill -INT $pid; wait $pid")
}

#[cfg(unix)]
#[test]
fn btrecv_without_args_listens_and_stops_cleanly() {
    let env = Env::new();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(sigint_script("btrecv", "--json"))
        .env("BT_HOME", env.home())
        .env("BT_BACKEND", "mock")
        .env_remove("APPDATA")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("sh 可执行");
    assert_eq!(code(&output), 0, "stderr={}", String::from_utf8_lossy(&output.stderr));
    let value: Value = serde_json::from_slice(&output.stdout).expect("退出时输出 JSON");
    assert_eq!(value["stopped"], true);
    assert_eq!(value["listening"], false);

    let plain = std::process::Command::new("sh")
        .arg("-c")
        .arg(sigint_script("btrecv", ""))
        .env("BT_HOME", env.home())
        .env("BT_BACKEND", "mock")
        .env_remove("APPDATA")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("sh 可执行");
    assert_eq!(code(&plain), 0, "stderr={}", String::from_utf8_lossy(&plain.stderr));
    assert!(String::from_utf8_lossy(&plain.stdout).contains("接收服务已停止"));
}

#[test]
fn bare_subcommands_default_to_list() {
    let env = Env::new();
    let devices = json(&run(&env, "bt", &["devices", "--json"]));
    assert_eq!(devices["count"], 2);

    let records = json(&run(&env, "bt", &["records", "--json"]));
    assert_eq!(records["count"], 0);
    assert_eq!(records["limit"], 100);

    let config = json(&run(&env, "bt", &["config", "--json"]));
    assert_eq!(config["config"]["opp_chunk_size"], 65023);
}

#[test]
fn btrecv_hidden_help_and_version_subcommands() {
    let env = Env::new();
    let help = run(&env, "btrecv", &["help"]);
    assert_eq!(code(&help), 0);
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage: btrecv"));

    let version = run(&env, "btrecv", &["version"]);
    assert_eq!(code(&version), 0);
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("btrecv "));

    let recv_help = run(&env, "btrecv", &["recv", "--help"]);
    assert_eq!(code(&recv_help), 0);
    assert!(String::from_utf8_lossy(&recv_help.stdout).contains("--no-ask"));
}

#[test]
fn error_json_includes_hint_and_type() {
    let env = Env::new();
    let output = run(&env, "bt", &["records", "delete", "999", "--json"]);
    assert_eq!(code(&output), 5);
    let value = json(&output);
    assert_eq!(value["type"], "error");
    assert_eq!(value["command"], "records.delete");
    assert!(value["error"]["hint"].as_str().unwrap().contains("records list"));
}

#[test]
fn send_rejects_directory_as_file() {
    let env = Env::new();
    let dir = env.path("not-a-file");
    std::fs::create_dir_all(&dir).unwrap();
    let output = run(&env, "bt", &["send", "file", PHONE, dir.to_str().unwrap(), "--json"]);
    assert_eq!(code(&output), 2);
    assert_eq!(json(&output)["error"]["kind"], "invalid_argument");
}

#[test]
fn password_rejects_command_line_value() {
    let env = Env::new();
    let output = run(
        &env,
        "bt",
        &["config", "set", "auth-password", "leaked-value", "--json"],
    );
    assert_eq!(code(&output), 2);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("leaked-value"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("leaked-value"));
}

#[test]
fn usage_errors_are_json_when_requested() {
    let env = Env::new();
    let output = run(&env, "bt", &["send", "--json"]);
    assert_eq!(code(&output), 2);
    let value = json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["type"], "error");
    assert_eq!(value["error"]["kind"], "invalid_argument");
    assert!(value["error"]["hint"].as_str().unwrap().contains("--help"));
}
