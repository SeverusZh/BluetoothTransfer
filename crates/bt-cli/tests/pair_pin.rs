use std::sync::Arc;

use bt_api::{PinPromptKind, SharedBackend};
use bt_app::AppPaths;
use bt_cli::backend::Context;
use bt_cli::cli::PairArgs;
use bt_cli::commands;
use bt_cli::error::EXIT_USAGE;
use bt_cli::output::OutputMode;

const PHONE: &str = "AA:BB:CC:00:00:01";

fn context() -> (tempfile::TempDir, Arc<bt_api::MockBackend>, Context) {
    let root = tempfile::tempdir().expect("临时目录");
    let paths = AppPaths::under(root.path());
    paths.create_dirs().expect("应用目录");
    let mock = Arc::new(bt_app::mock_backend(&paths).expect("模拟后端"));
    let backend: SharedBackend = mock.clone();
    let events = backend.subscribe();
    let ctx = Context {
        backend,
        events,
        paths,
        mode: OutputMode::Json,
        repl: false,
    };
    (root, mock, ctx)
}

#[test]
fn pair_forwards_pin_to_answer_pin() {
    let (_root, mock, ctx) = context();
    mock.set_pair_prompt(PinPromptKind::Pin);
    let args = PairArgs {
        address: PHONE.to_string(),
        pin: Some("1234".to_string()),
    };
    let code = commands::devices::pair(&ctx, &args).expect("PIN 配对应成功");
    assert_eq!(code, 0);
    assert!(mock.has_call("answer_pin"));
    assert!(!mock.has_call("answer_prompt"));
}

#[test]
fn pair_without_pin_reports_usage_error_for_pin_prompt() {
    let (_root, mock, ctx) = context();
    mock.set_pair_prompt(PinPromptKind::Pin);
    let args = PairArgs {
        address: PHONE.to_string(),
        pin: None,
    };
    let error = commands::devices::pair(&ctx, &args).expect_err("缺少 PIN 必须失败");
    assert_eq!(error.exit_code(), EXIT_USAGE);
    assert!(error.message().contains("--pin"));
    assert!(!mock.has_call("answer_pin"));
}

#[test]
fn pin_never_appears_in_mock_call_details() {
    let (_root, mock, ctx) = context();
    mock.set_pair_prompt(PinPromptKind::Pin);
    let args = PairArgs {
        address: PHONE.to_string(),
        pin: Some("987654".to_string()),
    };
    commands::devices::pair(&ctx, &args).expect("PIN 配对应成功");
    let calls = mock.take_calls();
    assert!(calls.iter().all(|call| !call.detail().contains("987654")));
}
