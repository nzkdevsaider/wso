#![cfg(windows)]

mod common;

use std::path::PathBuf;
use winshortcuts_organizer::ops::{apply, scan_owned, summarize};
use winshortcuts_organizer::plan::{Action, DesiredShortcut};
use winshortcuts_organizer::platform::Platform;
use winshortcuts_organizer::shortcut::{
    signature::{encode, Signature},
    writer::{write_lnk, ComGuard},
    LnkData,
};

fn system_exe() -> PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(windir).join("explorer.exe")
}

/// The whole safety guarantee of this tool rests on `scan_owned` claiming
/// only shortcuts carrying the `[wso:...]` marker. Alongside the signed
/// shortcut, this test plants every other shape `scan_owned` might see in a
/// real shortcuts folder: a hand-made shortcut with no marker, a plain
/// non-`.lnk` file, a `.lnk` file that is not valid shell-link data at all,
/// and a directory that happens to end in `.lnk`. Only the first may be
/// claimed; a bug that claimed any of the others would be exactly the bug
/// that makes deletion unsafe.
#[test]
fn scan_owned_claims_only_the_signed_shortcut_and_skips_everything_else() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("scan_owned");
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    write_lnk(
        &dir.join("Terraria.lnk"),
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    // A shortcut the user made by hand. It must never be claimed.
    write_lnk(
        &dir.join("My Notes.lnk"),
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "My own shortcut".into(),
        },
    )
    .unwrap();

    // Not a shortcut at all.
    std::fs::write(dir.join("readme.txt"), b"not a shortcut").unwrap();

    // A `.lnk` extension but not real shell-link data. Must be skipped, not
    // abort the scan.
    std::fs::write(dir.join("Broken.lnk"), b"this is not a real lnk file").unwrap();

    // A directory that happens to end in `.lnk`. Must never be read as a file.
    std::fs::create_dir_all(dir.join("Weird.lnk")).unwrap();

    let owned = scan_owned(std::slice::from_ref(&dir));
    assert_eq!(owned.len(), 1, "only the signed shortcut is ours");
    assert_eq!(owned[0].sig, sig);

    std::fs::remove_dir_all(&dir).ok();
}

/// Windows filesystems are case-insensitive, so a shortcut saved with an
/// uppercase `.LNK` extension (which File Explorer will happily create, e.g.
/// via a rename) is exactly as real a shell link as a lowercase one. If
/// `scan_owned` matched the extension case-sensitively, this file would be
/// invisible to it despite carrying our signature: never claimed, so
/// `clean` could never remove it and `sync` would create a duplicate
/// shortcut beside it.
#[test]
fn scan_owned_claims_a_signed_shortcut_with_an_uppercase_extension() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("uppercase_extension");
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "440".into(),
    };
    write_lnk(
        &dir.join("Team Fortress 2.LNK"),
        &LnkData {
            target: system_exe(),
            args: "-applaunch 440".into(),
            working_dir: None,
            icon: None,
            description: encode("Team Fortress 2", &sig),
        },
    )
    .unwrap();

    let owned = scan_owned(std::slice::from_ref(&dir));
    assert_eq!(
        owned.len(),
        1,
        "an uppercase .LNK extension must still be claimed"
    );
    assert_eq!(owned[0].sig, sig);

    std::fs::remove_dir_all(&dir).ok();
}

/// A dry run must be genuinely inert: no file written, and — since the real
/// path here sits inside a directory that does not exist yet — not even the
/// parent directory may be created. Only after a real (non-dry) run does
/// anything appear on disk.
#[test]
fn dry_run_creates_nothing_including_the_parent_directory() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("dry_run");
    let nested = dir.join("Nested");
    let target = nested.join("Game.lnk");

    let actions = vec![Action::Create(DesiredShortcut {
        sig: Signature {
            platform: Platform::Steam,
            id: "1".into(),
        },
        path: target.clone(),
        lnk: LnkData {
            target: system_exe(),
            args: "-applaunch 1".into(),
            working_dir: None,
            icon: None,
            description: "Game [wso:steam:1]".into(),
        },
    })];

    let outcomes = apply(&actions, true);
    assert_eq!(outcomes.len(), 1);
    assert!(
        !dir.exists(),
        "dry run must not create even the base directory"
    );
    assert!(!target.exists(), "dry run must not write");

    let outcomes = apply(&actions, false);
    assert!(target.is_file(), "a real run must write");
    assert_eq!(summarize(&outcomes).created, 1);
    assert_eq!(summarize(&outcomes).failed, 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn delete_removes_the_file() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("delete");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Old.lnk");

    write_lnk(
        &lnk,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "Old [wso:steam:9]".into(),
        },
    )
    .unwrap();
    assert!(lnk.is_file());

    let outcomes = apply(&[Action::DeleteOrphan(lnk.clone())], false);
    assert!(!lnk.exists());
    assert_eq!(summarize(&outcomes).deleted, 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// One item failing must not stop the rest of the batch: the first Create
/// here is forced to fail (its path sits under a plain file, so the
/// directory `write_lnk` would need to create can never exist), while the
/// second Create has nothing standing in its way. Only the mechanism that
/// keeps processing every action, regardless of an earlier failure, can
/// produce both "one failed" and "one created" from the same `apply` call.
#[test]
fn one_failed_action_does_not_stop_the_rest_of_the_batch() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("partial_failure");
    std::fs::create_dir_all(&dir).unwrap();

    let blocker = dir.join("blocker.txt");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let unwritable = blocker.join("subdir").join("Bad.lnk");
    let good = dir.join("Good.lnk");

    let actions = vec![
        Action::Create(DesiredShortcut {
            sig: Signature {
                platform: Platform::Steam,
                id: "1".into(),
            },
            path: unwritable.clone(),
            lnk: LnkData {
                target: system_exe(),
                args: "-applaunch 1".into(),
                working_dir: None,
                icon: None,
                description: "Bad [wso:steam:1]".into(),
            },
        }),
        Action::Create(DesiredShortcut {
            sig: Signature {
                platform: Platform::Steam,
                id: "2".into(),
            },
            path: good.clone(),
            lnk: LnkData {
                target: system_exe(),
                args: "-applaunch 2".into(),
                working_dir: None,
                icon: None,
                description: "Good [wso:steam:2]".into(),
            },
        }),
    ];

    let outcomes = apply(&actions, false);
    assert_eq!(outcomes.len(), 2);
    assert!(!unwritable.exists());
    assert!(
        good.is_file(),
        "the second action must still run despite the first failing"
    );

    let summary = summarize(&outcomes);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.created, 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// `Move` is two steps: write the new file, then remove the old one. Here
/// the old path is a directory (not a file), so `remove_file` on it is
/// guaranteed to fail while the write to the new path succeeds. The new
/// shortcut must survive (rolling it back would trade a recoverable
/// leftover for total data loss), the blocked old path must be left alone
/// rather than silently ignored, and the outcome must be reported as a
/// failure so an incomplete move is visible and can be retried rather than
/// quietly counted as done.
#[test]
fn move_reports_failure_when_the_old_path_cannot_be_removed_but_keeps_the_new_file() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("move_partial_failure");
    std::fs::create_dir_all(&dir).unwrap();

    let from = dir.join("Locked.lnk");
    std::fs::create_dir_all(&from).unwrap(); // a directory, not a file
    let to = dir.join("Game.lnk");

    let actions = vec![Action::Move {
        from: from.clone(),
        to: to.clone(),
        lnk: LnkData {
            target: system_exe(),
            args: "-applaunch 1".into(),
            working_dir: None,
            icon: None,
            description: "Game [wso:steam:1]".into(),
        },
    }];

    let outcomes = apply(&actions, false);
    assert_eq!(outcomes.len(), 1);
    assert!(
        to.is_file(),
        "the new shortcut must exist even though cleanup of the old path failed"
    );
    assert!(
        from.is_dir(),
        "the blocked old path is left in place, not silently ignored"
    );

    let summary = summarize(&outcomes);
    assert_eq!(
        summary.failed, 1,
        "an incomplete move must be reported as a failure"
    );
    assert_eq!(summary.moved, 0);

    std::fs::remove_dir_all(&dir).ok();
}
