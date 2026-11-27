use crate::cli::{Cli, Command, DevicesCommand, SendCommand};
use crate::error::CliError;

pub mod config;
pub mod devices;
pub mod export;
pub mod migrate;
pub mod records;
pub mod recv;
pub mod repl;
pub mod resume;
pub mod selftest;
pub mod send;

pub fn repl_entry(cli: &Cli) -> i32 {
    repl::run(cli)
}

pub fn dispatch(command: &Command, ctx: &crate::backend::Context) -> Result<u8, CliError> {
    match command {
        Command::Devices { action } => match action {
            None => devices::list(
                ctx,
                &crate::cli::ListArgs {
                    sort: crate::cli::SortArg::Last,
                },
            ),
            Some(DevicesCommand::List(args)) => devices::list(ctx, args),
            Some(DevicesCommand::Scan(args)) => devices::scan(ctx, args),
            Some(DevicesCommand::Pair(args)) => devices::pair(ctx, args),
            Some(DevicesCommand::Alias(args)) => devices::alias(ctx, args),
            Some(DevicesCommand::Favorite(args)) => devices::favorite(ctx, args),
            Some(DevicesCommand::Detect(args)) => devices::detect(ctx, args),
        },
        Command::Send { action } => match action {
            SendCommand::File(args) | SendCommand::Files(args) => send::file(ctx, args),
            SendCommand::Text(args) => send::text(ctx, args),
            SendCommand::Folder(args) => send::folder(ctx, args),
        },
        Command::Recv(args) => recv::run(ctx, args),
        Command::Records { action } => records::dispatch(ctx, action.as_ref()),
        Command::Export(args) => export::run(ctx, args),
        Command::Config { action } => config::dispatch(ctx, action.as_ref()),
        Command::Resume { action } => resume::dispatch(ctx, action),
        Command::Migrate(args) => migrate::run(ctx, args),
        Command::Selftest => selftest::run(ctx),
        Command::Gui => Err(CliError::failed("GUI 由入口直接处理")),
        Command::Repl => Err(CliError::failed("repl 由入口直接处理")),
        Command::OppScan(args) => devices::scan(ctx, args),
        Command::OppPair(args) => devices::pair(ctx, args),
        Command::OppSendFile(args) | Command::OppSendFiles(args) => send::file(ctx, args),
        Command::OppSendText(args) => send::text(ctx, args),
        Command::OppSendFolder(args) => send::folder(ctx, args),
        Command::Detect(args) => devices::detect(ctx, args),
        Command::Stats => records::stats(ctx),
    }
}
