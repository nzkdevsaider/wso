use std::process::Command;

#[test]
fn version_flag_reports_the_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_wso"))
        .arg("--version")
        .output()
        .expect("the built binary should run");

    assert!(output.status.success(), "--version should exit 0");

    let printed = String::from_utf8(output.stdout).expect("--version output should be UTF-8");
    let printed = printed.trim();
    let prefix = format!("wso {}", env!("CARGO_PKG_VERSION"));

    assert!(
        printed.starts_with(&prefix),
        "expected --version to start with {prefix:?}, got {printed:?}"
    );

    let suffix = &printed[prefix.len()..];
    assert!(
        suffix.is_empty() || is_development_suffix(suffix),
        "expected a bare version or a -dev.<sha> suffix, got {suffix:?}"
    );
}

#[test]
fn only_a_hex_sha_counts_as_a_development_suffix() {
    assert!(is_development_suffix("-dev.a1b2c3d"));
    assert!(!is_development_suffix("-dev."));
    assert!(!is_development_suffix("-dev.notahex"));
    assert!(!is_development_suffix(".a1b2c3d"));
}

fn is_development_suffix(suffix: &str) -> bool {
    suffix
        .strip_prefix("-dev.")
        .is_some_and(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
}
