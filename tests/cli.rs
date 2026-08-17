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
    assert!(stdout.contains("FOOTHOLD ESTABLISHED"));
    assert!(stdout.contains("FIRST CONTACT COMPLETE"));
    assert!(!stdout.contains("OPERATION FAILED"));
    assert!(!stdout.contains("FIRST CONTACT INCOMPLETE"));
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

    assert!(stdout.contains("tick  1 | drone (0, 0) | action: scan | budget remaining: 14"));
    assert!(stdout.contains("tick  2 | drone (0, 1) | action: north | budget remaining: 13"));
}

#[test]
fn the_example_script_scans_before_navigating() {
    let output = bin()
        .arg(example_path("first_contact.lua"))
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("action: scan"),
        "expected the reference controller to scan as part of its successful run, got: {stdout}"
    );
}

#[test]
fn each_tick_is_preceded_by_a_satellite_view_of_discovered_terrain() {
    let output = bin()
        .arg(example_path("first_contact.lua"))
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Every satellite frame header and every tick telemetry line, in the
    // order they were printed: this proves the two counts match (one frame
    // per tick, not just at least one) and that each frame immediately
    // precedes its tick's line, not just appears somewhere in the output.
    let events: Vec<&str> = stdout
        .lines()
        .filter(|line| *line == "SATELLITE FEED // discovered terrain" || line.starts_with("tick "))
        .collect();

    assert!(!events.is_empty(), "expected at least one tick to run");
    assert_eq!(
        events.len() % 2,
        0,
        "expected a matched satellite frame for every tick line, got {events:?}"
    );
    for pair in events.chunks(2) {
        assert_eq!(pair[0], "SATELLITE FEED // discovered terrain");
        assert!(
            pair[1].starts_with("tick "),
            "expected a tick line immediately after each satellite frame, got {:?}",
            pair[1]
        );
    }

    assert!(
        stdout
            .contains("legend: D drone   U uplink   . floor   # wall   ~ hazard   ? undiscovered")
    );
    // The first frame reflects tick 1's completed opening scan, which
    // reveals a 5x5 area around the drone's starting tile (0, 0).
    assert!(stdout.contains("y=0 |   D   #   #   ?   ?"));
}

#[test]
fn a_hazard_route_script_reports_the_hazard_telemetry_line_and_lower_final_budget() {
    let output = bin()
        .arg(fixture_path("hazard_route.lua"))
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("FOOTHOLD ESTABLISHED"));
    assert!(stdout.contains("tick  6 | drone (4, 2) | action: north | budget remaining: 4"));
    assert!(stdout.contains("hazard triggered at (4, 2): -5 budget"));
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
    assert!(stdout.contains("FIRST CONTACT INCOMPLETE"));
    assert!(!stdout.contains("FOOTHOLD ESTABLISHED"));
    assert!(!stdout.contains("FIRST CONTACT COMPLETE"));
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
