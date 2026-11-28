#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match bt_gui::main_entry(args) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(3)
        }
    }
}
