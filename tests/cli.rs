use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_human-exception"))
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
    assert!(stderr.contains("HUMAN EXCEPTION // resistance console"));
    assert!(stderr.contains("--bogus"));
    assert!(stderr.contains("--help"));
    assert!(!stdout.contains("System bootstrap complete"));
}
