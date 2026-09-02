use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, FromArgMatches, Parser};

use human_exception::{Action, FailureReason, SimEvent, TickOutcome, TickRecord};

const BANNER: &str = "HUMAN EXCEPTION // resistance console";

#[derive(Parser, Debug)]
#[command(
    name = "human-exception",
    about = "Boot the resistance console and begin an operation. Direct script \
             execution (--developer-mode) is contributor/test tooling, not the \
             normal way to play.",
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

    /// Run a Lua script directly against the fixed scenario, bypassing the
    /// resistance console. Contributor/test tooling only: not a normal way
    /// to play, and it does not read or affect normal Player progression.
    /// Requires a script path.
    #[arg(long)]
    developer_mode: bool,

    /// Path to the Lua script that will control the operation. Only used
    /// with --developer-mode.
    script: Option<PathBuf>,
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

            match (cli.developer_mode, cli.script) {
                (true, Some(script)) => {
                    println!("{BANNER}");
                    std::process::exit(run_operation(&script));
                }
                (true, None) => reject_usage(
                    &mut command,
                    "--developer-mode requires a script path to run",
                ),
                (false, Some(_)) => reject_usage(
                    &mut command,
                    "direct script execution requires --developer-mode; it is \
                     contributor/test tooling, not a normal way to play",
                ),
                (false, None) => std::process::exit(enter_console()),
            }
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

/// Prints an in-character usage-error report for a semantically invalid
/// combination of otherwise-well-formed arguments (e.g. a script path
/// without `--developer-mode`), matching the format clap's own parse
/// errors are rendered in above, and exits with status 2.
fn reject_usage(command: &mut clap::Command, detail: &str) -> ! {
    eprintln!("{BANNER}");
    eprintln!("Uplink rejected: {detail}");
    eprintln!();
    eprint!("{}", command.render_usage());
    eprintln!();
    eprintln!("Try 'human-exception --help' for the field manual.");
    std::process::exit(2);
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

/// Enters the persistent resistance-console session when a real terminal is
/// attached, or prints the bootstrap banner and returns immediately when it
/// isn't (piped/redirected stdio, as in noninteractive CI invocations).
/// Returns the process exit code.
fn enter_console() -> i32 {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        println!("{BANNER}");
        println!("No active satellite link. System bootstrap complete.");
        return 0;
    }

    match human_exception::console::run() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{BANNER}");
            eprintln!("Uplink rejected: console session failed: {err}");
            3
        }
    }
}

/// Runs the reconnaissance operation controlled by the script at `script`,
/// printing tick-by-tick telemetry and a final report, and returns the
/// process exit code: `0` on success, `1` on mission failure, `3` if the
/// script could not be loaded or executed.
fn run_operation(script: &Path) -> i32 {
    println!("Uplink script: {}", script.display());
    println!();

    let mut tick_count = 0u32;
    let result = human_exception::lua_controller::run(script, |record| {
        tick_count = record.tick;
        println!(
            "{}",
            human_exception::render_satellite_view(
                record.drone_position,
                record.map_width,
                record.map_height,
                &record.discovered,
            )
        );
        println!("{}", format_tick_line(&record));
        for line in format_event_lines(&record) {
            println!("{line}");
        }
        println!();
    });

    // Wording mirrors the interactive console's After Action outcome
    // hierarchy (`docs/TUI_DESIGN.md` §5, `src/console/ui.rs`
    // FOOTHOLD_ESTABLISHED_*/FIRST_CONTACT_*/NO_FURTHER_OPERATION*
    // constants) in order — outcome, meaning, completion, then next
    // actions/availability: reaching the uplink establishes a resistance
    // foothold in the facility network, not capture/ownership/control, and
    // no follow-on operation exists at this facility either way.
    match result {
        Ok(TickOutcome::Succeeded) => {
            println!();
            println!(
                "FOOTHOLD ESTABLISHED after {tick_count} tick(s). Resistance access to the facility network was established. FIRST CONTACT COMPLETE. No further operation is available at this facility."
            );
            0
        }
        Ok(TickOutcome::Failed(reason)) => {
            println!();
            println!(
                "OPERATION FAILED: {}. No facility foothold was established. FIRST CONTACT INCOMPLETE.",
                format_failure(reason)
            );
            1
        }
        Ok(TickOutcome::Running) => {
            unreachable!("lua_controller::run only returns once the operation has ended")
        }
        Err(err) => {
            eprintln!();
            eprintln!("Uplink rejected: {err}");
            3
        }
    }
}

fn format_tick_line(record: &TickRecord) -> String {
    format!(
        "tick {:>2} | drone ({}, {}) | action: {} | budget remaining: {}",
        record.tick,
        record.drone_position.x,
        record.drone_position.y,
        format_action(record.action),
        record.budget_remaining,
    )
}

fn format_event_lines(record: &TickRecord) -> Vec<String> {
    record
        .events
        .iter()
        .filter_map(|event| match event {
            SimEvent::HazardEntered { position, amount } => Some(format!(
                "  hazard triggered at ({}, {}): -{amount} budget",
                position.x, position.y
            )),
            _ => None,
        })
        .collect()
}

fn format_action(action: Action) -> &'static str {
    match action {
        Action::MoveNorth => "north",
        Action::MoveSouth => "south",
        Action::MoveEast => "east",
        Action::MoveWest => "west",
        Action::Wait => "wait",
        Action::Scan => "scan",
    }
}

fn format_failure(reason: FailureReason) -> String {
    match reason {
        FailureReason::BudgetExhausted => {
            "the operation ran out of budget before reaching the uplink".to_string()
        }
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
