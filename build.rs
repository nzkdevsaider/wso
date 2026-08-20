use std::path::PathBuf;
use std::process::Command;

fn main() {
    let package_version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    println!("cargo:rustc-env=WSO_VERSION={}", version(&package_version));
    for path in rerun_paths() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn version(package_version: &str) -> String {
    match head_commit() {
        Some(sha) if !head_is_tagged(package_version) => format!("{package_version}-dev.{sha}"),
        _ => package_version.to_owned(),
    }
}

fn head_commit() -> Option<String> {
    git(&["rev-parse", "--short=7", "HEAD"])
}

fn head_is_tagged(package_version: &str) -> bool {
    let release_tag = format!("v{package_version}");
    git(&["tag", "--points-at", "HEAD"])
        .is_some_and(|tags| tags.lines().any(|tag| tag.trim() == release_tag))
}

fn rerun_paths() -> Vec<PathBuf> {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return Vec::new();
    };
    let mut paths = vec![
        git_dir.join("HEAD"),
        git_dir.join("packed-refs"),
        git_dir.join("refs").join("tags"),
    ];
    if let Some(head_ref) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        paths.push(git_dir.join(head_ref));
    }
    paths.retain(|path| path.exists());
    paths
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}
