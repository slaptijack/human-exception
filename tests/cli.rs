use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_human-exception"))
}

fn example_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn no_arguments_prints_startup_banner() {
    let output = bin().output().expect("binary should run");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "HUMAN EXCEPTION // resistance console\nNo active satellite link. System bootstrap complete.\n"
    );
}

#[test]
fn long_help_flag_prints_in_character_usage() {
    let output = bin().arg("--help").output().expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("HUMAN EXCEPTION // resistance console"));
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--version"));
}

#[test]
fn short_help_flag_prints_in_character_usage() {
    let output = bin().arg("-h").output().expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("HUMAN EXCEPTION // resistance console"));
    assert!(stdout.contains("Usage:"));
}

#[test]
fn long_version_flag_prints_package_version() {
    let output = bin().arg("--version").output().expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("HUMAN EXCEPTION // resistance console"));
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn short_version_flag_prints_package_version() {
    let output = bin().arg("-V").output().expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_argument_is_rejected_in_character() {
    let output = bin().arg("--bogus").output().expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("HUMAN EXCEPTION // resistance console"));
    assert!(stderr.contains("--bogus"));
    assert!(stderr.contains("--help"));
    assert!(!stdout.contains("System bootstrap complete"));
}

#[test]
fn running_the_example_script_succeeds() {
    let output = bin()
        .arg(example_path("first_contact.lua"))
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("UPLINK ESTABLISHED"));
    assert!(!stdout.contains("OPERATION FAILED"));
}

#[test]
fn running_the_example_script_is_deterministic() {
    let first = bin()
        .arg(example_path("first_contact.lua"))
        .output()
        .expect("binary should run");
    let second = bin()
        .arg(example_path("first_contact.lua"))
        .output()
        .expect("binary should run");

    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.status.code(), second.status.code());
}

#[test]
fn each_tick_reports_position_action_and_remaining_time() {
    let output = bin()
        .arg(example_path("first_contact.lua"))
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("tick  1 | drone (0, 1) | action: north | uplink in 19 tick(s)"));
}

#[test]
fn a_wait_only_script_reports_mission_failure() {
    let output = bin()
        .arg(fixture_path("always_wait.lua"))
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("OPERATION FAILED"));
    assert!(!stdout.contains("UPLINK ESTABLISHED"));
}

#[test]
fn a_nonexistent_script_prints_a_readable_error_and_exits_nonzero() {
    let output = bin()
        .arg(fixture_path("does_not_exist.lua"))
        .output()
        .expect("binary should run");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(3));
    assert!(stderr.contains("Uplink rejected:"));
    assert!(stderr.contains("could not read script"));
}

#[test]
fn a_malformed_script_prints_a_readable_error_and_exits_nonzero() {
    let output = bin()
        .arg(fixture_path("syntax_error.lua"))
        .output()
        .expect("binary should run");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(3));
    assert!(stderr.contains("Uplink rejected:"));
    assert!(stderr.contains("script failed to load"));
}

#[test]
fn a_script_missing_the_callback_prints_a_readable_error() {
    let output = bin()
        .arg(fixture_path("missing_callback.lua"))
        .output()
        .expect("binary should run");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(3));
    assert!(stderr.contains("on_tick"));
}
