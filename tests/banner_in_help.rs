use winshortcutsorganizer::banner::{header, BANNER, VERSION};

use std::process::Command;

#[test]
fn help_carries_the_banner_line_for_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_wso"))
        .arg("--help")
        .output()
        .expect("the built binary should run");

    assert!(output.status.success(), "--help should exit 0");

    let help = String::from_utf8(output.stdout).expect("--help output should be UTF-8");

    for line in header().lines() {
        assert!(
            help.lines().any(|rendered| rendered == line),
            "clap dropped or reflowed the banner line {line:?}"
        );
    }
}

#[test]
fn help_shows_the_version_under_the_art() {
    let output = Command::new(env!("CARGO_BIN_EXE_wso"))
        .arg("--help")
        .output()
        .expect("the built binary should run");

    let help = String::from_utf8(output.stdout).expect("--help output should be UTF-8");
    let lines: Vec<&str> = help.lines().collect();

    let last_art_line = BANNER.lines().next_back().expect("the banner has lines");
    let art_at = lines
        .iter()
        .position(|line| *line == last_art_line)
        .expect("the art should survive into --help");

    assert!(
        lines[art_at + 1].contains(VERSION),
        "the line under the art should carry the version, got {:?}",
        lines[art_at + 1]
    );
}

#[test]
fn a_redirected_run_prints_no_banner() {
    let output = Command::new(env!("CARGO_BIN_EXE_wso"))
        .args(["clean", "--dry-run"])
        .output()
        .expect("the built binary should run");

    let printed = String::from_utf8_lossy(&output.stdout);

    assert!(
        !printed.contains(BANNER.lines().next().expect("the banner has lines")),
        "a piped run should print no banner, got {printed:?}"
    );
}
