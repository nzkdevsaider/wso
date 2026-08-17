#![cfg(windows)]

//! Spec §9: "Switching modes on a later run produces `Move` actions rather
//! than duplicates."
//!
//! This exercises the real pipeline a `sync` run uses — scan the disk, build
//! what should exist, reconcile — rather than `plan()` alone. `plan()` has
//! always emitted a `Move` when a signature is found at the wrong path; the
//! defect was upstream of it, in *which directories were scanned*, so a
//! test that hands `plan()` a ready-made `owned` list cannot see the bug at
//! all. Only planting a real shortcut on disk under the old layout and
//! letting the scan find it (or not) can.

mod common;

use std::path::PathBuf;
use winshortcuts_organizer::config::Config;
use winshortcuts_organizer::layout::{self, Organization};
use winshortcuts_organizer::ops;
use winshortcuts_organizer::plan::{plan, Action};
use winshortcuts_organizer::platform::{Game, IconCandidate, Launch, Platform};
use winshortcuts_organizer::shortcut::{
    signature::{encode, Signature},
    writer::{write_lnk, ComGuard},
    LnkData,
};

fn system_exe() -> PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(windir).join("explorer.exe")
}

fn terraria() -> Game {
    Game {
        platform: Platform::Steam,
        id: "105600".into(),
        name: "Terraria".into(),
        install_dir: PathBuf::from(r"C:\Games\Terraria"),
        launch: Launch {
            program: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
        },
        icon_candidates: vec![IconCandidate::Fallback(system_exe())],
    }
}

/// The confirmed defect: synced as `flat`, then re-run as `unified`, and
/// nine new shortcuts appeared in the folder while the nine loose ones
/// remained — every game duplicated, and the originals stranded where no
/// later run configured for `unified` would ever look at them again.
///
/// The shortcut is planted in the flat layout (directly under `base`) while
/// the configuration asks for `Unified { "Games" }`. A scan of only the
/// configured layout looks solely in `base\Games`, finds nothing, and so
/// emits a bare `Create` — the planted file is never even a candidate for a
/// `Move`, and never becomes an orphan either, because `plan()` cannot
/// delete what was never scanned. Only widening the scan to the layouts a
/// previous run could have produced can turn this into the single `Move`
/// asserted here.
#[test]
fn switching_from_flat_to_unified_moves_the_existing_shortcut_instead_of_duplicating_it() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_flat_to_unified");
    std::fs::create_dir_all(&base).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let planted = base.join("Terraria.lnk");
    write_lnk(
        &planted,
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    let cfg = Config {
        organization: Organization::Unified {
            folder: "Games".into(),
        },
        ..Default::default()
    };
    let cache = base.join("cache");

    let owned = ops::scan_owned(&layout::scan_dirs(&base, &cfg.organization));
    let desired = ops::build_desired(&[terraria()], &cfg, &base, &cache);
    let actions = plan(&desired, &owned);

    match actions.as_slice() {
        [Action::Move { from, to, .. }] => {
            assert_eq!(from, &planted);
            assert_eq!(to, &base.join("Games").join("Terraria.lnk"));
        }
        other => panic!("expected exactly one Move, got {other:?}"),
    }

    std::fs::remove_dir_all(&base).ok();
}

/// The reverse switch, which fails for the same reason in the other
/// direction: a shortcut sitting in `base\Games` from a `unified` run, with
/// the configuration now `Flat`. A scan of only the configured layout looks
/// solely in `base` itself and never descends into `Games`.
#[test]
fn switching_from_unified_to_flat_moves_the_existing_shortcut_instead_of_duplicating_it() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_unified_to_flat");
    std::fs::create_dir_all(&base).unwrap();

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let planted = base.join("Games").join("Terraria.lnk");
    write_lnk(
        &planted,
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    let cfg = Config {
        organization: Organization::Flat,
        ..Default::default()
    };
    let cache = base.join("cache");

    let owned = ops::scan_owned(&layout::scan_dirs(&base, &cfg.organization));
    let desired = ops::build_desired(&[terraria()], &cfg, &base, &cache);
    let actions = plan(&desired, &owned);

    match actions.as_slice() {
        [Action::Move { from, to, .. }] => {
            assert_eq!(from, &planted);
            assert_eq!(to, &base.join("Terraria.lnk"));
        }
        other => panic!("expected exactly one Move, got {other:?}"),
    }

    std::fs::remove_dir_all(&base).ok();
}

/// Widening the scan must not widen what is *claimable*. A hand-made
/// shortcut sitting in a directory the widened scan now visits — but which
/// the configured layout would never have looked at — must still be
/// invisible: `scan_owned` gates on the signature regardless of which
/// directory a file sits in, which is the argument that makes this change
/// safe. Without that gate, the file below would become an owned shortcut
/// with no matching desired entry, and `plan()` would emit a `DeleteOrphan`
/// for it.
#[test]
fn widening_the_scan_does_not_make_an_unsigned_shortcut_eligible() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_unsigned_stays_foreign");
    std::fs::create_dir_all(&base).unwrap();

    let foreign = base.join("My Notes.lnk");
    write_lnk(
        &foreign,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "My own shortcut".into(),
        },
    )
    .unwrap();

    let cfg = Config {
        organization: Organization::Unified {
            folder: "Games".into(),
        },
        ..Default::default()
    };

    let dirs = layout::scan_dirs(&base, &cfg.organization);
    assert!(
        dirs.contains(&base),
        "the widened scan must actually visit the flat base, or this test proves nothing: {dirs:?}"
    );

    let owned = ops::scan_owned(&dirs);
    assert!(
        owned.is_empty(),
        "an unsigned shortcut must never be claimed, however many directories are scanned"
    );
    assert!(plan(&[], &owned).is_empty());

    std::fs::remove_dir_all(&base).ok();
}
