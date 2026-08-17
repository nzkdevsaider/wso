#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use winshortcuts_organizer::shortcut::{
    writer::{read_lnk, write_lnk, ComGuard},
    LnkData,
};

/// Windows resolves a shortcut's local-file target and working directory to
/// their actual on-disk casing when the link is saved (that's how shell
/// links stay valid after e.g. a case rename), so a test that hardcodes a
/// path built from an env var can fail on a machine whose `%WINDIR%` casing
/// (observed here as `C:\WINDOWS`) does not match the disk entry
/// (`C:\Windows`) — a filesystem quirk unrelated to `write_lnk`/`read_lnk`.
/// Canonicalizing the fixture path up front makes the expectation match
/// what Windows will actually resolve to, on any machine.
fn canonical(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).expect("fixture path must exist on disk");
    let s = c.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

/// The project's central premise is that a shortcut's icon location is
/// independent of its target: a game can launch through Steam's executable
/// while showing an icon from a completely different file. `target` is
/// pointed at `explorer.exe` and `icon` at `shell32.dll` — two different
/// real files — so a bug that derived the icon from the target (or vice
/// versa) would fail this test, not just one that dropped a field.
#[test]
fn round_trips_every_field_with_an_icon_independent_of_the_target() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Terraria.lnk");

    let windir = canonical(Path::new(
        &std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()),
    ));
    let explorer = windir.join("explorer.exe");
    let shell32 = windir.join("System32").join("shell32.dll");
    assert_ne!(
        explorer, shell32,
        "test fixture must use two distinct real files"
    );

    let data = LnkData {
        target: explorer.clone(),
        args: "-applaunch 105600".into(),
        working_dir: Some(windir.clone()),
        icon: Some((shell32.clone(), 4)),
        description: "Terraria — managed by WinShortcutsOrganizer [wso:steam:105600]".into(),
    };

    write_lnk(&lnk, &data).unwrap();
    assert!(lnk.is_file());

    let read = read_lnk(&lnk).unwrap();
    assert_eq!(read.target, explorer);
    assert_eq!(read.args, "-applaunch 105600");
    assert_eq!(read.working_dir, Some(windir.clone()));
    assert_eq!(read.icon, Some((shell32.clone(), 4)));
    assert_eq!(read.description, data.description);
    assert_ne!(
        read.target,
        read.icon.as_ref().unwrap().0,
        "icon location must survive the round trip as a file distinct from the target"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Steam/Epic-style launch arguments are a single quoted URI, not a
/// space-separated argument list. This proves `write_lnk`/`read_lnk` pass
/// that string through byte-for-byte rather than re-tokenizing or
/// re-quoting it.
#[test]
fn preserves_a_quoted_uri_argument_for_epic() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_uri");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Fall Guys.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");
    let uri = "\"com.epicgames.launcher://apps/a%3Ab%3Ac?action=launch&silent=true\"";

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: uri.into(),
            working_dir: None,
            icon: None,
            description: "Fall Guys — managed by WinShortcutsOrganizer [wso:epic:c]".into(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().args, uri);
    std::fs::remove_dir_all(&dir).ok();
}

/// This description does not carry the `[wso:...]` ownership marker — it
/// stands in for a value some other, non-owning code path put there. The
/// point is not that another application wrote the file (both the write
/// and the read here go through this crate's own functions); the point is
/// that `read_lnk` must not normalize, default, or otherwise alter a
/// description just because it doesn't match the marker shape the
/// ownership check (`shortcut::signature`) is looking for.
#[test]
fn round_trips_a_description_without_the_wso_marker_byte_for_byte() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_non_owned_desc");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("NotOurs.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");
    let non_owned_description = "Some Other Launcher v2.1 — do not touch";

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: String::new(),
            working_dir: None,
            icon: None,
            description: non_owned_description.into(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().description, non_owned_description);
    std::fs::remove_dir_all(&dir).ok();
}

/// `icon` and `working_dir` both branch on `is_empty()` on the read side
/// (an empty string becomes `None`), but `description` does not — it is a
/// plain `String`, and an empty one is a legitimate value (most shortcuts
/// this tool creates for platforms with no natural description source
/// will have one). This is the witness that the empty case for
/// `description` specifically is not silently turned into something else.
#[test]
fn round_trips_an_empty_description() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_empty_desc");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Empty.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: String::new(),
            working_dir: None,
            icon: None,
            description: String::new(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().description, "");
    std::fs::remove_dir_all(&dir).ok();
}

/// Measured directly (see the task report): a description of exactly 260
/// UTF-16 code units is the longest that `IShellLinkW::SetDescription`
/// preserves intact — this is the positive half of that boundary, proving
/// `write_lnk`'s limit constant matches what Windows actually allows
/// rather than an approximation of it.
#[test]
fn writes_and_reads_back_a_description_at_the_measured_shell_link_limit() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_desc_at_limit");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("AtLimit.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");
    let description = "d".repeat(260);

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: String::new(),
            working_dir: None,
            icon: None,
            description: description.clone(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().description, description);
    std::fs::remove_dir_all(&dir).ok();
}

/// One UTF-16 code unit past the measured 260-unit shell-link limit:
/// `write_lnk` must reject this rather than silently hand Windows a
/// description that `SetDescription` would then truncate on disk — the
/// whole point of validating up front is that nothing this tool writes
/// can come back different from what it wrote. No `.lnk` file may exist
/// afterward.
#[test]
fn write_lnk_rejects_a_description_one_unit_over_the_shell_link_limit_and_creates_no_file() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_desc_over_limit");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("OverLimit.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");

    let result = write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "d".repeat(261),
        },
    );

    assert!(
        result.is_err(),
        "a 261-unit description exceeds the measured 260-unit limit"
    );
    assert!(!lnk.exists(), "a rejected write must not create a file");
    std::fs::remove_dir_all(&dir).ok();
}

/// Measured directly: arguments have no Windows-imposed cap (a 32,000-unit
/// argument string round-trips fine given a large enough read buffer), so
/// the binding constraint is this crate's own read buffer, which needs one
/// code unit of headroom to null-terminate. This is the positive half of
/// that boundary — the longest argument string the current buffer size
/// can still read back exactly.
#[test]
fn writes_and_reads_back_arguments_at_the_buffer_limit() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_args_at_limit");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("AtLimit.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");
    let args = "a".repeat(4095);

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: args.clone(),
            working_dir: None,
            icon: None,
            description: String::new(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().args, args);
    std::fs::remove_dir_all(&dir).ok();
}

/// One code unit past the buffer's usable capacity: without validation,
/// `read_lnk` would silently hand back a 4095-unit argument string that
/// looks complete but is missing its last character (see the sibling test
/// above for the exact boundary) — a shortcut this tool wrote would then
/// fail to read back byte-for-byte. `write_lnk` must reject it instead,
/// and no `.lnk` file may exist afterward.
#[test]
fn write_lnk_rejects_arguments_one_unit_over_the_buffer_limit_and_creates_no_file() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_args_over_limit");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("OverLimit.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");

    let result = write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: "a".repeat(4096),
            working_dir: None,
            icon: None,
            description: String::new(),
        },
    );

    assert!(
        result.is_err(),
        "a 4096-unit argument string exceeds the buffer's 4095-unit limit"
    );
    assert!(!lnk.exists(), "a rejected write must not create a file");
    std::fs::remove_dir_all(&dir).ok();
}
