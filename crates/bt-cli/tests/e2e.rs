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

#[cfg(unix)]
fn run_with_home_real(program: &str, home: &Path, args: &[&str]) -> Output {
    let mut command = assert_cmd::Command::cargo_bin(program).expect("二进制存在");
    command
        .env("BT_HOME", home)
        .env_remove("BT_BACKEND")
        .env_remove("APPDATA")
        .env_remove("LOCALAPPDATA")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .args(args);
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
fn send_rejects_user_source_inside_pending() {
    let env = Env::new();
    let pending = env.path(".local/share/BluetoothTransfer/pending");
    std::fs::create_dir_all(&pending).unwrap();
    let source = pending.join("user-source.txt");
    std::fs::write(&source, b"user data").unwrap();

    for extra in [Vec::new(), vec!["--zip"]] {
        let mut args = vec!["send", "file", PHONE, source.to_str().unwrap(), "--json"];
        args.extend(extra.iter().copied());
        let output = run(&env, "bt", &args);
        assert_eq!(code(&output), 2, "stderr={}", String::from_utf8_lossy(&output.stderr));
        let value = json(&output);
        assert_eq!(value["type"], "error");
        assert_eq!(value["error"]["kind"], "invalid_argument");
        let hint = value["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("pending"), "hint 需指出 pending：{hint}");
        assert!(hint.contains("移出"), "hint 需可操作：{hint}");
        assert!(source.is_file(), "拒绝不得删除源文件");
    }
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

#[test]
fn zip_rejects_4gib_source_and_leaves_no_artifact() {
    let env = Env::new();
    let huge = env.path("huge.bin");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(4 * 1024 * 1024 * 1024).unwrap();
    let output = run(
        &env,
        "bt",
        &["send", "file", PHONE, huge.to_str().unwrap(), "--zip", "--json"],
    );
    assert_eq!(code(&output), 1, "stderr={}", String::from_utf8_lossy(&output.stderr));
    let value = json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["type"], "error");
    assert_eq!(value["error"]["kind"], "unsupported");
    assert!(value["error"]["hint"].as_str().unwrap().contains("4 GiB"));
    let pending = env.path(".local/share/BluetoothTransfer/pending");
    if pending.is_dir() {
        assert_eq!(
            std::fs::read_dir(&pending).unwrap().count(),
            0,
            "失败后不得残留打包产物"
        );
    }
}

#[test]
fn cancel_keeps_resume_entry_and_resume_run_finishes_it() {
    use bt_api::{Backend, ConfigPatch, JobState, SendItem};
    use bt_core::{Channel, DeviceId, SendSource, TransferMode};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let env = Env::new();
    let paths = bt_app::AppPaths::portable(env.home());
    let transport = Arc::new(bt_transport::mock::MockTransport::new(Vec::new()));
    let backend =
        bt_app::RealBackend::open_with_transport_options(paths.clone(), Some(transport.clone()), false).unwrap();
    backend
        .update_config(ConfigPatch {
            mode: Some(TransferMode::Assistant),
            ..Default::default()
        })
        .unwrap();
    let gate = transport.gate_next_connect();
    let item = SendItem::from_source(SendSource::Text("cancel-resume".to_string()));
    let device = DeviceId::parse(PHONE).unwrap();
    let job = backend.enqueue(&device, vec![item], Some(Channel::Assistant)).unwrap()[0];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = bt_app::list_resume(&paths).unwrap();
        if entries.iter().any(|entry| entry.state == "sending") {
            break;
        }
        assert!(Instant::now() < deadline, "journal 条目未写入：{entries:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    backend.cancel_all_jobs().unwrap();
    gate.release();
    loop {
        if backend
            .jobs()
            .iter()
            .any(|snapshot| snapshot.id == job && snapshot.state == JobState::Canceled)
        {
            break;
        }
        assert!(Instant::now() < deadline, "任务未进入取消终态：{:?}", backend.jobs());
        std::thread::sleep(Duration::from_millis(20));
    }
    backend.shutdown();
    drop(backend);

    let listed = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(code(&listed), 0);
    let value = json(&listed);
    assert_eq!(value["count"], 1, "取消后 resume list 必须能看到条目");
    assert_eq!(value["entries"][0]["state"], "paused");

    let resumed = run(&env, "bt", &["resume", "run", "--json"]);
    assert_eq!(code(&resumed), 0, "stderr={}", String::from_utf8_lossy(&resumed.stderr));
    let value = json(&resumed);
    assert_eq!(value["resumed"], 1);
    assert_eq!(value["summary"]["completed"], 1);
    assert_eq!(value["jobs"][0]["item"]["display_name"], "bt-note.txt");

    let after = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(json(&after)["count"], 0, "续传完成后应清理条目");
    let pending = env.path(".local/share/BluetoothTransfer/pending");
    if pending.is_dir() {
        assert_eq!(
            std::fs::read_dir(&pending).unwrap().count(),
            0,
            "续传完成后应清理自产载荷"
        );
    }
}

#[test]
fn zip_cancel_keeps_payload_and_resume_run_finishes_it() {
    use bt_api::Backend;
    use bt_cli::backend::Context;
    use bt_cli::cli::SendPathsArgs;
    use bt_cli::output::OutputMode;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let env = Env::new();
    let paths = bt_app::AppPaths::portable(env.home());
    paths.create_dirs().unwrap();
    let source = env.path("zip-source.txt");
    std::fs::write(&source, b"zip-resume-payload").unwrap();

    let transport = Arc::new(bt_transport::mock::MockTransport::new(Vec::new()));
    let gate = transport.gate_next_connect();
    let backend = Arc::new(
        bt_app::RealBackend::open_with_transport_options(paths.clone(), Some(transport.clone()), false).unwrap(),
    );
    let shared: bt_api::SharedBackend = backend.clone();
    let ctx = Context {
        backend: shared,
        events: backend.subscribe(),
        paths: paths.clone(),
        mode: OutputMode::Json,
        repl: false,
    };
    let args = SendPathsArgs {
        device: PHONE.to_string(),
        paths: vec![source.clone()],
        zip: true,
        mode: None,
    };
    let handle = std::thread::spawn(move || bt_cli::commands::send::file(&ctx, &args));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = bt_app::list_resume(&paths).unwrap();
        if entries.iter().any(|entry| entry.state == "sending") {
            break;
        }
        assert!(Instant::now() < deadline, "zip journal 条目未写入：{entries:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    backend.cancel_all_jobs().unwrap();
    gate.release();
    assert_eq!(handle.join().unwrap().unwrap(), 1, "取消发送以 1 退出");
    backend.shutdown();

    let entries = bt_app::list_resume(&paths).unwrap();
    assert_eq!(entries.len(), 1, "取消后 resume list 必须能看到条目");
    assert_eq!(entries[0].state, "paused");
    let payload = entries[0].payload.clone();
    assert!(payload.is_file(), "取消后自产 zip 必须保留：{}", payload.display());
    assert!(payload.starts_with(&paths.pending_dir), "自产 zip 位于 pending 目录");
    assert!(source.is_file(), "用户源文件不得被删除");

    let content = std::fs::read(&payload).unwrap();
    let sha = bt_core::hex_lower(&bt_core::sha256(&content));
    let mut inbound = Vec::new();
    inbound.extend_from_slice(
        &bt_engine::protocol::encode_offer(&bt_engine::protocol::Offer::accept(0))
            .unwrap()
            .encode(),
    );
    inbound.extend_from_slice(&bt_engine::protocol::encode_ack(content.len() as i64).unwrap().encode());
    inbound.extend_from_slice(
        &bt_engine::protocol::encode_done(&bt_engine::protocol::Done::new(true, sha))
            .unwrap()
            .encode(),
    );
    let resume_transport = Arc::new(bt_transport::mock::MockTransport::new(Vec::new()));
    resume_transport.script_connect(inbound);
    let resume_backend = Arc::new(
        bt_app::RealBackend::open_with_transport_options(paths.clone(), Some(resume_transport), false).unwrap(),
    );
    let shared: bt_api::SharedBackend = resume_backend.clone();
    let resume_ctx = Context {
        backend: shared,
        events: resume_backend.subscribe(),
        paths: paths.clone(),
        mode: OutputMode::Json,
        repl: false,
    };
    assert_eq!(
        bt_cli::commands::resume::run(&resume_ctx).unwrap(),
        0,
        "resume run 应完成续传"
    );
    resume_backend.shutdown();
    assert!(bt_app::list_resume(&paths).unwrap().is_empty(), "完成后 journal 清空");
    assert!(!payload.exists(), "续传完成后删除自产 zip");
    assert!(source.is_file(), "用户源文件不受影响");

    let packed = bt_app::pack_zip_file_in(&paths, &source).unwrap();
    let bytes = std::fs::read(&packed.path).unwrap();
    let entry = bt_engine::JournalEntry::new(
        bt_core::DeviceId::parse(PHONE).unwrap(),
        packed.display_name.clone(),
        packed.path.clone(),
        bt_core::sha256(&bytes),
        bytes.len() as u64,
        bt_engine::JournalKind::FileZip,
        packed.owned,
    );
    let journal = bt_engine::Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
    let id = journal.begin(entry).unwrap().entry.id_hex();
    journal.pause(&id).unwrap();
    let cancel_backend =
        Arc::new(bt_app::RealBackend::open_with_transport_options(paths.clone(), None, false).unwrap());
    let shared: bt_api::SharedBackend = cancel_backend.clone();
    let cancel_ctx = Context {
        backend: shared,
        events: cancel_backend.subscribe(),
        paths: paths.clone(),
        mode: OutputMode::Json,
        repl: false,
    };
    assert_eq!(bt_cli::commands::resume::cancel(&cancel_ctx).unwrap(), 0);
    cancel_backend.shutdown();
    assert!(!packed.path.exists(), "显式 resume cancel 清理自产 zip");
    assert!(source.is_file());
}

#[test]
fn send_folder_rejects_zip32_limit_with_unsupported_error() {
    let env = Env::new();
    let folder = env.path("huge-folder");
    std::fs::create_dir_all(&folder).unwrap();
    let huge = folder.join("huge.bin");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(4 * 1024 * 1024 * 1024).unwrap();
    drop(file);

    let output = run(
        &env,
        "bt",
        &["send", "folder", PHONE, folder.to_str().unwrap(), "--json"],
    );
    assert_eq!(code(&output), 1, "stderr={}", String::from_utf8_lossy(&output.stderr));
    let value = json(&output);
    assert_eq!(value["type"], "error");
    assert_eq!(value["error"]["kind"], "unsupported");
    let hint = value["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("拆分"), "hint 需给出可操作建议：{hint}");
    assert!(hint.contains("4 GiB"));
    let pending = env.path(".local/share/BluetoothTransfer/pending");
    if pending.is_dir() {
        assert_eq!(
            std::fs::read_dir(&pending).unwrap().count(),
            0,
            "预检失败不得残留打包产物"
        );
    }
}

#[test]
fn repl_help_emits_structured_rows() {
    let env = Env::new();
    let output = run_with_stdin(&env, "bt", &["repl", "--json"], Some("help\n?\nquit\n"));
    assert_eq!(code(&output), 0, "stderr={}", String::from_utf8_lossy(&output.stderr));
    let lines = ndjson(&output);
    assert_eq!(lines.len(), 2, "help 与 ? 各输出一个对象：{lines:?}");
    for line in &lines {
        assert_eq!(line["ok"], true);
        assert_eq!(line["command"], "repl.help");
        assert!(line["commands"].as_array().unwrap().len() >= 7);
    }

    let events = run_with_stdin(&env, "bt", &["repl", "--events"], Some("help\nquit\n"));
    assert_eq!(code(&events), 0);
    let lines = ndjson(&events);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["type"], "result");
    assert_eq!(lines[0]["ok"], true);
}

fn run_with_scripted_transport(env: &Env, args: &[&str]) -> Output {
    let mut command = assert_cmd::Command::cargo_bin("bt").expect("二进制存在");
    command
        .env("BT_HOME", env.home())
        .env("BT_TEST_TRANSPORT", "mock")
        .env_remove("BT_BACKEND")
        .env_remove("APPDATA")
        .env_remove("LOCALAPPDATA")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .args(args);
    command.output().expect("命令可执行")
}

#[test]
fn real_backend_resumes_only_for_transfer_commands() {
    let env = Env::new();
    let (id, payload) = seed_journal(&env, "real-resume.bin", b"real-journal", false);
    let journal = env.path(".local/share/BluetoothTransfer/journal/send-journal.bin");
    let before = std::fs::read(&journal).unwrap();

    let scan = run_with_scripted_transport(&env, &["devices", "scan", "--seconds", "1", "--json"]);
    assert_eq!(code(&scan), 0, "stderr={}", String::from_utf8_lossy(&scan.stderr));
    assert_eq!(std::fs::read(&journal).unwrap(), before, "只读命令不得改写 journal");

    let send = run_with_scripted_transport(
        &env,
        &["send", "text", PHONE, "resume-trigger", "--mode", "assistant", "--json"],
    );
    assert_eq!(code(&send), 1, "脚本化传输无入站数据，发送应失败");
    assert_ne!(
        std::fs::read(&journal).unwrap(),
        before,
        "send 路径必须触发自动续传并改写 journal"
    );

    let listed = run_with_scripted_transport(&env, &["resume", "list", "--json"]);
    assert_eq!(code(&listed), 0);
    let entries = json(&listed);
    let entries = entries["entries"].as_array().unwrap();
    let seeded = entries.iter().find(|entry| entry["id"] == id).expect("续传条目仍在");
    assert!(
        seeded["attempts"].as_u64().unwrap() >= 1,
        "send 应尝试续传原条目：{seeded}"
    );
    assert!(payload.is_file(), "续传载荷保持原样");
}

#[cfg(unix)]
#[test]
fn cli_sigint_keeps_zip_payload_for_resume() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let env = Env::new();
    let source = env.path("sigint-source.txt");
    std::fs::write(&source, b"sigint zip payload").unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_bt"))
        .env("BT_HOME", env.home())
        .env("BT_TEST_TRANSPORT", "stall")
        .env_remove("BT_BACKEND")
        .env_remove("APPDATA")
        .env_remove("LOCALAPPDATA")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .args(["send", "file", PHONE, source.to_str().unwrap(), "--zip", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 bt");

    let paths = bt_app::AppPaths::portable(env.home());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let entries = bt_app::list_resume(&paths).unwrap_or_default();
        if entries.iter().any(|entry| entry.state == "sending") {
            break;
        }
        assert!(Instant::now() < deadline, "CLI 未进入 sending：{entries:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let signal = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("发送 SIGINT");
    assert!(signal.success());
    let output = child.wait_with_output().expect("等待 bt 退出");
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout 应为单个 JSON 对象");
    assert_eq!(value["summary"]["canceled"], 1);
    assert_eq!(value["jobs"][0]["state"], "canceled");

    let entries = bt_app::list_resume(&paths).unwrap();
    assert_eq!(entries.len(), 1, "取消后 journal 条目必须保留");
    assert_eq!(entries[0].state, "paused");
    assert!(
        entries[0].payload.is_file(),
        "SIGINT 取消后自产 zip 必须保留：{}",
        entries[0].payload.display()
    );
    assert!(source.is_file(), "用户源文件不得被删除");
}

#[cfg(unix)]
#[test]
fn permission_errors_exit_4_with_permission_kind() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::new();
    let readonly = env.path("readonly-home");
    std::fs::create_dir_all(&readonly).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::write(readonly.join("probe"), b"x").is_ok();
    if writable {
        let _ = std::fs::remove_file(readonly.join("probe"));
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("跳过：当前用户对只读目录仍有写权限，无法触发权限错误");
        return;
    }
    let config = run_with_home_real("bt", &readonly, &["config", "set", "mode", "opp", "--json"]);
    let devices = run_with_home_real("bt", &readonly, &["devices", "list", "--json"]);
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755)).unwrap();
    for output in [&config, &devices] {
        assert_eq!(code(output), 4, "stderr={}", String::from_utf8_lossy(&output.stderr));
        let value = json(output);
        assert_eq!(value["type"], "error");
        assert_eq!(value["error"]["kind"], "permission");
        assert!(value["error"]["hint"].as_str().unwrap().contains("权限"));
    }

    let env = Env::new();
    let readonly = env.path("readonly-out");
    std::fs::create_dir_all(&readonly).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).unwrap();
    let target = readonly.join("records.csv");
    let export = run_with_home_real("bt", env.home(), &["export", "csv", target.to_str().unwrap(), "--json"]);
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code(&export), 4, "stderr={}", String::from_utf8_lossy(&export.stderr));
    assert_eq!(json(&export)["error"]["kind"], "permission");
}

#[test]
fn events_parse_errors_emit_ndjson_error() {
    let env = Env::new();
    let output = run(&env, "bt", &["frobnicate", "--events"]);
    assert_eq!(code(&output), 2);
    let lines = ndjson(&output);
    let last = lines.last().expect("events 模式必须输出错误行");
    assert_eq!(last["type"], "error");
    assert_eq!(last["ok"], false);
    assert_eq!(last["command"], "bt");
    assert_eq!(last["error"]["kind"], "invalid_argument");
    assert!(last["error"]["hint"].as_str().unwrap().contains("--help"));

    let missing = run(&env, "bt", &["send", "--events"]);
    assert_eq!(code(&missing), 2);
    assert_eq!(ndjson(&missing).last().unwrap()["type"], "error");

    let recv = run(&env, "btrecv", &["--bogus", "--events"]);
    assert_eq!(code(&recv), 2);
    let last = ndjson(&recv).pop().expect("btrecv events 必须输出错误行");
    assert_eq!(last["command"], "btrecv");
    assert_eq!(last["error"]["kind"], "invalid_argument");
}

#[test]
fn repl_json_reports_errors_as_objects() {
    let env = Env::new();
    let input = "frobnicate\ngui\nrecv\n\ndevices list --json\nquit\n";
    let output = run_with_stdin(&env, "bt", &["repl", "--json"], Some(input));
    assert_eq!(code(&output), 0);
    let lines = ndjson(&output);
    assert_eq!(lines.len(), 4, "每个非空命令输出一个对象：{lines:?}");
    assert_eq!(lines[0]["type"], "error");
    assert_eq!(lines[0]["error"]["kind"], "invalid_argument");
    assert_eq!(lines[1]["type"], "error");
    assert_eq!(lines[1]["command"], "gui");
    assert_eq!(lines[2]["type"], "error");
    assert_eq!(lines[2]["command"], "recv");
    assert_eq!(lines[3]["ok"], true);
}

#[test]
fn repl_events_reports_errors_as_ndjson() {
    let env = Env::new();
    let input = "frobnicate\ndevices list --json\nquit\n";
    let output = run_with_stdin(&env, "bt", &["repl", "--events"], Some(input));
    assert_eq!(code(&output), 0);
    let lines = ndjson(&output);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["type"], "error");
    assert_eq!(lines[1]["type"], "result");
    assert_eq!(lines[1]["count"], 2);
}

#[test]
fn repl_honors_line_level_events_on_parse_errors() {
    let env = Env::new();
    let output = run_with_stdin(&env, "bt", &["repl"], Some("frobnicate --events\nquit\n"));
    assert_eq!(code(&output), 0);
    let lines = ndjson(&output);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["type"], "error");
    assert_eq!(lines[0]["error"]["kind"], "invalid_argument");
}

#[test]
fn read_only_commands_do_not_touch_the_resume_journal() {
    let env = Env::new();
    let (id, _payload) = seed_journal(&env, "只读.bin", b"readonly-journal", false);
    let journal = env.path(".local/share/BluetoothTransfer/journal/send-journal.bin");
    let before = std::fs::read(&journal).unwrap();
    let export = env.path("records.jsonl");
    let commands: Vec<Vec<&str>> = vec![
        vec!["devices", "list", "--json"],
        vec!["records", "list", "--json"],
        vec!["records", "stats", "--json"],
        vec!["config", "list", "--json"],
        vec!["config", "set", "mode", "opp", "--json"],
        vec!["export", "jsonl", export.to_str().unwrap(), "--json"],
        vec!["resume", "list", "--json"],
        vec!["selftest", "--json"],
    ];
    for args in commands {
        let output = run(&env, "bt", &args);
        assert_eq!(
            code(&output),
            0,
            "{args:?} 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(&journal).unwrap(), before, "{args:?} 不得改写 journal");
    }
    let listed = run(&env, "bt", &["resume", "list", "--json"]);
    assert_eq!(json(&listed)["count"], 1);
    assert_eq!(json(&listed)["entries"][0]["id"], id);
}
