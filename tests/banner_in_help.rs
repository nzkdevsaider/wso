use winshortcutsorganizer::banner::BANNER;

use std::process::Command;

#[test]
fn help_carries_the_banner_line_for_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_wso"))
        .arg("--help")
        .output()
        .expect("the built binary should run");

    assert!(output.status.success(), "--help should exit 0");

    let help = String::from_utf8(output.stdout).expect("--help output should be UTF-8");

    for line in BANNER.lines() {
        assert!(
            help.lines().any(|rendered| rendered == line),
            "clap dropped or reflowed the banner line {line:?}"
        );
    }
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
