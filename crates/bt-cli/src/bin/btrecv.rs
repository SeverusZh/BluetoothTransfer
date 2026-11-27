fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(bt_cli::recv_entry() as u8)
}
