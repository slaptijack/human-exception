use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, FromArgMatches, Parser};

const BANNER: &str = "HUMAN EXCEPTION // resistance console";

#[derive(Parser, Debug)]
#[command(
    name = "human-exception",
    about = "Boot the console and begin an operation.",
    disable_version_flag = true,
    help_template = "\
HUMAN EXCEPTION // resistance console

{about}

{usage-heading} {usage}

{all-args}"
)]
struct Cli {
    /// Report this build's firmware version and exit
    #[arg(short = 'V', long)]
    version: bool,
}

/// Parses process arguments and drives the resistance-console startup
/// sequence, printing in-character help/version/error output and exiting
/// with an appropriate status code where the command doesn't proceed to
/// bootstrap.
pub fn run() {
    let mut command = Cli::command();

    match command.clone().try_get_matches_from(std::env::args_os()) {
        Ok(matches) => {
            let cli = match Cli::from_arg_matches(&matches) {
                Ok(cli) => cli,
                Err(e) => e.exit(),
            };

            if cli.version {
                println!("{BANNER}");
                println!("Firmware v{}", env!("CARGO_PKG_VERSION"));
                return;
            }

            println!("{BANNER}");
            println!("No active satellite link. System bootstrap complete.");
        }
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            e.exit();
        }
        Err(e) => {
            eprintln!("{BANNER}");
            eprintln!("Uplink rejected: {}", bad_argument_detail(&e));
            eprintln!();
            eprint!("{}", command.render_usage());
            eprintln!();
            eprintln!("Try 'human-exception --help' for the field manual.");
            std::process::exit(2);
        }
    }
}

fn bad_argument_detail(error: &clap::Error) -> String {
    let arg = error
        .get(ContextKind::InvalidArg)
        .or_else(|| error.get(ContextKind::InvalidSubcommand));

    match arg {
        Some(ContextValue::String(value)) => format!("unrecognized directive '{value}'"),
        _ => "unrecognized directive".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
