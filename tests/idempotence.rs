#![cfg(windows)]

//! Spec §12's idempotency property, checked the only way it can actually be
//! checked: through a real filesystem round trip.
//!
//! `plan.rs` used to carry a unit test named `is_idempotent` that called the
//! pure `plan()` twice with the same two slices and asserted the results had
//! equal lengths. A pure function given identical inputs returns identical
//! output by definition, so no change to `plan()` could have made that fail.
//! It proved nothing.
//!
//! The property that can fail is the round trip: a shortcut this tool writes
//! must, when read back off disk and reconciled against the very same
//! `desired` entry that produced it, come back `Skip` — not `Retarget`, not
//! `FixIcon`. Everything that could break it lives between `write_lnk` and
//! `read_lnk`: Windows resolving a target's on-disk casing, an icon path
//! stored differently from the way it was supplied, a description that did
//! not survive intact. None of that is visible without touching a disk.
//!
//! A failure here is not cosmetic. `plan()` would emit the same `Retarget`
//! on every run forever — rewriting shortcuts that were already correct,
//! reporting churn that never converges, and burying any genuine change in
//! the noise.

mod common;

use std::path::PathBuf;
use winshortcutsorganizer::config::Config;
use winshortcutsorganizer::layout::Organization;
use winshortcutsorganizer::ops;
use winshortcutsorganizer::plan::{plan, Action};
use winshortcutsorganizer::platform::{Game, IconCandidate, Launch, Platform};
use winshortcutsorganizer::shortcut::writer::ComGuard;

fn windir() -> PathBuf {
    PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()))
}

/// Two games, so a bug that happens to cancel out for one entry has a second
/// chance to show. Both launch through a real executable and carry a real
/// fallback icon, because the whole point is that these values survive a
/// save/load cycle through the Windows shell.
fn games() -> Vec<Game> {
    let explorer = windir().join("explorer.exe");
    vec![
        Game {
            platform: Platform::Steam,
            id: "105600".into(),
            name: "Terraria".into(),
            install_dir: PathBuf::from(r"C:\Games\Terraria"),
            launch: Launch {
                program: explorer.clone(),
                args: "-applaunch 105600".into(),
                working_dir: Some(windir()),
            },
            icon_candidates: vec![IconCandidate::Fallback(
                windir().join("System32").join("shell32.dll"),
            )],
        },
        Game {
            platform: Platform::Epic,
            id: "Fortnite".into(),
            name: "Fall Guys".into(),
            install_dir: PathBuf::from(r"D:\Epic\FallGuys"),
            launch: Launch {
                program: explorer.clone(),
                args: "\"com.epicgames.launcher://apps/ns%3Acid%3AFortnite?action=launch\"".into(),
                working_dir: None,
            },
            icon_candidates: vec![IconCandidate::Fallback(explorer)],
        },
    ]
}

fn assert_all_skip(actions: &[Action], stage: &str) {
    assert!(
        !actions.is_empty(),
        "{stage}: nothing was reconciled at all, so this proves nothing"
    );
    for action in actions {
        assert!(
            matches!(action, Action::Skip { .. }),
            "{stage}: a shortcut written by this tool and read straight back must be up to \
             date, got {action:?}"
        );
    }
}

/// Write the shortcuts, read them back, re-plan the identical `desired`
/// against what came off disk: every action must be a `Skip`. Then do it once
/// more, so a first pass that quietly corrected something would be caught by
/// the second.
#[test]
fn a_shortcut_written_and_read_back_needs_no_further_change() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("idempotence_roundtrip");
    std::fs::create_dir_all(&base).unwrap();
    let cache = base.join("cache");

    let cfg = Config {
        organization: Organization::Unified {
            folder: "Games".into(),
        },
        platforms: vec![Platform::Steam, Platform::Epic],
        ..Default::default()
    };
    let games = games();

    let desired = ops::build_desired(&games, &cfg, &base, &cache);
    assert_eq!(desired.len(), 2, "both games must become desired shortcuts");

    let outcomes = ops::apply(&plan(&desired, &[]), false);
    let summary = ops::summarize(&outcomes);
    assert_eq!(summary.failed, 0, "the initial write must succeed outright");
    assert_eq!(summary.created, 2);

    for stage in ["first re-plan", "second re-plan"] {
        let owned = ops::scan_owned(&ops::scan_dirs(&base));
        assert_eq!(
            owned.len(),
            2,
            "{stage}: both shortcuts must be found again on disk"
        );
        let desired = ops::build_desired(&games, &cfg, &base, &cache);
        assert_all_skip(&plan(&desired, &owned), stage);
    }

    std::fs::remove_dir_all(&base).ok();
}

/// The same property with the target spelled the way a provider might
/// legitimately hand it over — lowercased, with forward slashes — while the
/// shortcut on disk was written from the canonical spelling. This is the
/// mechanism `plan::same_path` exists for, exercised end to end: without
/// case- and separator-insensitive comparison, this run reports a `Retarget`
/// that the next run would report again, and again, forever.
#[test]
fn a_target_supplied_in_a_different_spelling_still_reconciles_as_up_to_date() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("idempotence_respelled_target");
    std::fs::create_dir_all(&base).unwrap();
    let cache = base.join("cache");

    let cfg = Config {
        organization: Organization::Flat,
        platforms: vec![Platform::Steam],
        ..Default::default()
    };

    let canonical = windir().join("explorer.exe");
    let mut game = games().remove(0);
    game.launch.program = canonical.clone();
    game.launch.working_dir = Some(windir());

    let outcomes = ops::apply(
        &plan(
            &ops::build_desired(&[game.clone()], &cfg, &base, &cache),
            &[],
        ),
        false,
    );
    assert_eq!(ops::summarize(&outcomes).created, 1);

    // The same executable, spelled the way a registry value or a hand-edited
    // config might hold it.
    let respelled = canonical
        .to_string_lossy()
        .to_lowercase()
        .replace('\\', "/");
    game.launch.program = PathBuf::from(&respelled);
    game.launch.working_dir = Some(PathBuf::from(
        windir().to_string_lossy().to_lowercase().replace('\\', "/"),
    ));
    assert_ne!(
        game.launch.program, canonical,
        "the respelled path must actually differ as a string, or this test proves nothing"
    );

    let owned = ops::scan_owned(&ops::scan_dirs(&base));
    let desired = ops::build_desired(&[game], &cfg, &base, &cache);
    assert_all_skip(&plan(&desired, &owned), "respelled target");

    std::fs::remove_dir_all(&base).ok();
}
