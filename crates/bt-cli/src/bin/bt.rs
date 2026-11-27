fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = bt_cli::main_entry(args);
    std::process::ExitCode::from(code.clamp(0, 255) as u8)
}
