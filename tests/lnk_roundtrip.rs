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
    assert_ne!(explorer, shell32, "test fixture must use two distinct real files");

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

/// `read_lnk` must return a foreign shortcut's description exactly as
/// written, with no normalization or default substitution — the ownership
/// check downstream depends on comparing this string byte-for-byte against
/// the `[wso:...]` marker format.
#[test]
fn reads_back_a_foreign_description_unmodified() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("lnk_foreign_desc");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("NotOurs.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");
    let foreign_description = "Some Other Launcher v2.1 — do not touch";

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: String::new(),
            working_dir: None,
            icon: None,
            description: foreign_description.into(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().description, foreign_description);
    std::fs::remove_dir_all(&dir).ok();
}
