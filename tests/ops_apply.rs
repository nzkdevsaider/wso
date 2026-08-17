#![cfg(windows)]

mod common;

use std::path::PathBuf;
use winshortcuts_organizer::ops::{apply, scan_owned, summarize};
use winshortcuts_organizer::plan::{Action, DesiredShortcut};
use winshortcuts_organizer::platform::Platform;
use winshortcuts_organizer::shortcut::{
    signature::{decode, encode, Signature},
    writer::{read_lnk, write_lnk, ComGuard},
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

/// C1, the `Create` half. `README.md` promises that `sync` "never modifies
/// or removes a file it did not create"; before this test existed, a plain
/// `Create` called `write_lnk` unconditionally and a hand-made
/// `Terraria.lnk` at the destination was silently replaced — target,
/// description and all — while the run reported a successful create and
/// zero failures. There is no Recycle Bin on that path: the user's file is
/// simply gone.
///
/// The planted shortcut carries no `[wso:...]` marker, so only an ownership
/// check at the write site can produce both halves of this assertion: the
/// bytes unchanged *and* a reported failure. A gate that merely refused to
/// write whenever anything exists at the destination would also pass this
/// one, which is why
/// `create_overwrites_a_shortcut_this_tool_already_owns_for_the_same_game`
/// sits beside it.
#[test]
fn create_refuses_to_overwrite_a_foreign_shortcut_and_reports_it_as_a_failure() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("create_collision_foreign");
    std::fs::create_dir_all(&dir).unwrap();

    let planted = dir.join("Terraria.lnk");
    write_lnk(
        &planted,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "MIO - no tocar".into(),
        },
    )
    .unwrap();
    let original_bytes = std::fs::read(&planted).unwrap();

    let actions = vec![Action::Create(DesiredShortcut {
        sig: Signature {
            platform: Platform::Steam,
            id: "105600".into(),
        },
        path: planted.clone(),
        lnk: LnkData {
            target: dir.join("steam.exe"),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode(
                "Terraria",
                &Signature {
                    platform: Platform::Steam,
                    id: "105600".into(),
                },
            ),
        },
    })];

    let outcomes = apply(&actions, false);

    assert!(
        std::fs::read(&planted).unwrap() == original_bytes,
        "a shortcut this tool did not create must be left byte-for-byte untouched"
    );
    let summary = summarize(&outcomes);
    assert_eq!(
        summary.failed, 1,
        "a blocked collision must be reported as a failed item, not a silent success"
    );
    assert_eq!(summary.created, 0);

    std::fs::remove_dir_all(&dir).ok();
}

/// The other side of C1's gate: it is a *collision* check, not a blanket
/// refusal to write where a file already exists. A leftover carrying our own
/// signature for this same game is functionally the shortcut this run would
/// write anyway, so overwriting it must still succeed — and must actually
/// rewrite the file, which the target assertion proves.
#[test]
fn create_overwrites_a_shortcut_this_tool_already_owns_for_the_same_game() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("create_collision_owned");
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let path = dir.join("Terraria.lnk");
    write_lnk(
        &path,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    let new_target = dir.join("steam.exe");
    std::fs::write(&new_target, b"stand-in for steam.exe").unwrap();
    let actions = vec![Action::Create(DesiredShortcut {
        sig: sig.clone(),
        path: path.clone(),
        lnk: LnkData {
            target: new_target.clone(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    })];

    let outcomes = apply(&actions, false);

    let summary = summarize(&outcomes);
    assert_eq!(summary.failed, 0, "our own shortcut is safe to refresh");
    assert_eq!(summary.created, 1);
    assert_eq!(
        read_lnk(&path).unwrap().target,
        new_target,
        "the file must actually have been rewritten, not merely left alone"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// C1, the `Move` half. A mode switch relocates a shortcut by writing it at
/// the new path, and that write went through the same unguarded `write_lnk`
/// as `Create`. Here a hand-made file already sits at the destination: it
/// must survive, the shortcut being moved must stay where it is (a move that
/// could not land must not also destroy its own source), and the run must
/// report the failure.
#[test]
fn move_refuses_to_overwrite_a_foreign_shortcut_at_the_destination() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("move_collision_foreign");
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let from = dir.join("Terraria.lnk");
    write_lnk(
        &from,
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    let to = dir.join("Games").join("Terraria.lnk");
    write_lnk(
        &to,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "MIO - no tocar".into(),
        },
    )
    .unwrap();
    let original_bytes = std::fs::read(&to).unwrap();

    let actions = vec![Action::Move {
        from: from.clone(),
        to: to.clone(),
        lnk: LnkData {
            target: dir.join("steam.exe"),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    }];

    let outcomes = apply(&actions, false);

    assert!(
        std::fs::read(&to).unwrap() == original_bytes,
        "the foreign shortcut at the destination must be left byte-for-byte untouched"
    );
    assert!(
        from.is_file(),
        "a move that could not land must not remove the shortcut it was moving"
    );
    let summary = summarize(&outcomes);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.moved, 0);

    std::fs::remove_dir_all(&dir).ok();
}

/// `Move` is two steps — write the new file, then remove the old one — and
/// the second step was guarded by `from != to`, a byte-exact comparison on a
/// case-insensitive filesystem. When the two spellings name the *same* file,
/// `write_lnk` overwrites it in place and `remove_file(from)` then deletes
/// exactly what was just written. The run reports `1 moved, 0 failed`, and
/// the shortcut is gone.
///
/// The folder-case route became reachable with the filesystem-derived scan:
/// `ops::scan_dirs` returns the on-disk spelling (`Mis Juegos`) while
/// `build_desired` uses the configured one (`mis juegos`), so `plan()` emits
/// a `Move` between two names for one directory. The collision gate cannot
/// catch this — it correctly reports `OwnedBySame`, and the unguarded
/// operation is the deletion, not the write.
#[test]
fn a_move_between_two_spellings_of_the_same_folder_keeps_the_shortcut() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("move_same_folder_other_case");
    let on_disk = dir.join("Mis Juegos");
    std::fs::create_dir_all(&on_disk).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let from = on_disk.join("Terraria.lnk");
    let lnk = LnkData {
        target: system_exe(),
        args: "-applaunch 105600".into(),
        working_dir: None,
        icon: None,
        description: encode("Terraria", &sig),
    };
    write_lnk(&from, &lnk).unwrap();

    // The same directory, spelled the way the configuration now holds it.
    let to = dir.join("mis juegos").join("Terraria.lnk");
    assert_ne!(from, to, "the two paths must differ as strings");

    let outcomes = apply(
        &[Action::Move {
            from: from.clone(),
            to: to.clone(),
            lnk,
        }],
        false,
    );

    assert!(
        to.is_file(),
        "the shortcut must survive a move between two spellings of its own folder"
    );
    assert_eq!(
        decode(&read_lnk(&to).unwrap().description),
        Some(sig),
        "and must still be the shortcut this tool owns"
    );
    assert_eq!(
        std::fs::read_dir(&on_disk).unwrap().count(),
        1,
        "exactly one shortcut, neither deleted nor duplicated"
    );

    let summary = summarize(&outcomes);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.moved, 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// The same defect reached through the filename rather than the folder, which
/// is not new: a user who renames `Terraria.lnk` to `TERRARIA.lnk` has always
/// been able to reach it. One file on disk, two spellings, and the deletion
/// removes the file the write just produced.
#[test]
fn a_move_between_two_spellings_of_the_same_filename_keeps_the_shortcut() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("move_same_file_other_case");
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let from = dir.join("Terraria.lnk");
    let lnk = LnkData {
        target: system_exe(),
        args: "-applaunch 105600".into(),
        working_dir: None,
        icon: None,
        description: encode("Terraria", &sig),
    };
    write_lnk(&from, &lnk).unwrap();

    let to = dir.join("TERRARIA.lnk");
    assert_ne!(from, to, "the two paths must differ as strings");

    let outcomes = apply(
        &[Action::Move {
            from: from.clone(),
            to: to.clone(),
            lnk,
        }],
        false,
    );

    assert!(to.is_file(), "the shortcut must survive");
    assert_eq!(decode(&read_lnk(&to).unwrap().description), Some(sig));
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "exactly one shortcut, neither deleted nor duplicated"
    );

    let summary = summarize(&outcomes);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.moved, 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// The guard on the two tests above: skipping the deletion is only correct
/// when the two paths are the same file. A genuine relocation must still
/// leave nothing behind, or a mode switch would accumulate a duplicate for
/// every game instead of moving it — which is the defect C2 exists to fix.
/// A `remove_file` deleted outright would pass both tests above and fail this
/// one.
#[test]
fn a_move_between_genuinely_different_paths_still_removes_the_source() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("move_different_paths_removes_source");
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let from = dir.join("Terraria.lnk");
    let lnk = LnkData {
        target: system_exe(),
        args: "-applaunch 105600".into(),
        working_dir: None,
        icon: None,
        description: encode("Terraria", &sig),
    };
    write_lnk(&from, &lnk).unwrap();

    let to = dir.join("Mis Juegos").join("Terraria.lnk");

    let outcomes = apply(
        &[Action::Move {
            from: from.clone(),
            to: to.clone(),
            lnk,
        }],
        false,
    );

    assert!(to.is_file(), "the shortcut must exist at its new path");
    assert!(
        !from.exists(),
        "a genuine relocation must not leave the original behind"
    );

    let summary = summarize(&outcomes);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.moved, 1);

    std::fs::remove_dir_all(&dir).ok();
}
