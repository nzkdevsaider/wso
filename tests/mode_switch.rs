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
//!
//! **Every folder name in this file is deliberately not `"Games"`.** The
//! first attempt at this fix derived the directories to scan from the
//! configured layout plus the literal default `"Games"`, and every fixture
//! here used `"Games"` — so the tests passed while the real case failed.
//! `Cli::apply_overrides` replaces `cfg.organization` wholesale when
//! `--organize flat` is passed, discarding the previous folder name, so by
//! the time anything looks at the configuration there is nothing left to
//! remember. A name-derived scan cannot work; these fixtures are named so
//! that a regression to one fails here.

mod common;

use std::path::{Path, PathBuf};
use winshortcuts_organizer::config::Config;
use winshortcuts_organizer::layout::Organization;
use winshortcuts_organizer::ops;
use winshortcuts_organizer::plan::{plan, Action};
use winshortcuts_organizer::platform::{Game, IconCandidate, Launch, Platform};
use winshortcuts_organizer::shortcut::{
    signature::{encode, Signature},
    writer::{write_lnk, ComGuard},
    LnkData,
};

/// A folder name that is not the default, contains a space, and is not
/// ASCII-only — nothing about it can be guessed from the configuration once
/// the configuration has been replaced.
const FOLDER: &str = "Mis Juegos";

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

fn plant_owned_shortcut(path: &Path) {
    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    write_lnk(
        path,
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();
}

/// Runs the reconciliation half of a real `sync`: scan the disk for shortcuts
/// this tool owns, build what should exist under `organization`, reconcile.
fn reconcile(base: &Path, organization: Organization) -> Vec<Action> {
    let cfg = Config {
        organization,
        ..Default::default()
    };
    let owned = ops::scan_owned(&ops::scan_dirs(base));
    let desired = ops::build_desired(&[terraria()], &cfg, base, &base.join("cache"));
    plan(&desired, &owned)
}

fn assert_single_move(actions: &[Action], from: &Path, to: &Path) {
    match actions {
        [Action::Move {
            from: got_from,
            to: got_to,
            ..
        }] => {
            assert_eq!(got_from, from);
            assert_eq!(got_to, to);
        }
        other => panic!("expected exactly one Move, got {other:?}"),
    }
}

/// The confirmed defect, in the shape that actually occurs: synced into a
/// folder, then re-run with `--organize flat`. Nine new shortcuts appeared
/// loose on the Desktop while the nine in the folder remained.
///
/// The folder is not named `Games`, which is what makes this the real case
/// rather than the lucky one. `--organize flat` sets `cfg.organization` to
/// `Organization::Flat`, which carries no folder name at all, so nothing in
/// the configuration can lead a scan back to `Mis Juegos`. Only looking at
/// what is actually on disk can.
#[test]
fn switching_away_from_a_folder_the_configuration_no_longer_names_moves_the_shortcut() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_forgotten_folder");
    std::fs::create_dir_all(&base).unwrap();

    let planted = base.join(FOLDER).join("Terraria.lnk");
    plant_owned_shortcut(&planted);

    let actions = reconcile(&base, Organization::Flat);
    assert_single_move(&actions, &planted, &base.join("Terraria.lnk"));

    std::fs::remove_dir_all(&base).ok();
}

/// The same switch in the other direction: loose on the Desktop from a flat
/// run, now configured for a unified folder whose name the previous run knew
/// nothing about.
#[test]
fn switching_from_flat_into_a_folder_moves_the_existing_shortcut() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_flat_into_folder");
    std::fs::create_dir_all(&base).unwrap();

    let planted = base.join("Terraria.lnk");
    plant_owned_shortcut(&planted);

    let actions = reconcile(
        &base,
        Organization::Unified {
            folder: FOLDER.into(),
        },
    );
    assert_single_move(&actions, &planted, &base.join(FOLDER).join("Terraria.lnk"));

    std::fs::remove_dir_all(&base).ok();
}

/// Switching between two *different* folder names, neither of which is the
/// default. Nothing in the configuration names the old one, and the default
/// `"Games"` is not involved on either side — so a scan that consulted any
/// remembered name, however cleverly, finds nothing here.
#[test]
fn switching_between_two_folders_the_configuration_never_names_together_moves_the_shortcut() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_between_two_folders");
    std::fs::create_dir_all(&base).unwrap();

    let planted = base.join("Viejos Juegos").join("Terraria.lnk");
    plant_owned_shortcut(&planted);

    let actions = reconcile(
        &base,
        Organization::Unified {
            folder: FOLDER.into(),
        },
    );
    assert_single_move(&actions, &planted, &base.join(FOLDER).join("Terraria.lnk"));

    std::fs::remove_dir_all(&base).ok();
}

/// The per-platform layout nests one level deeper than any other:
/// `PerPlatform { parent: Some("Mis Juegos") }` writes into
/// `base\Mis Juegos\Steam`. A scan bounded at "base plus its immediate
/// subdirectories" reaches `base\Mis Juegos` and stops one directory short of
/// the shortcut — so this is the case that decides how deep the scan has to
/// go, and it is a layout the tool itself produces, not an exotic one.
#[test]
fn switching_away_from_a_nested_per_platform_folder_moves_the_shortcut() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_nested_per_platform");
    std::fs::create_dir_all(&base).unwrap();

    let planted = base.join(FOLDER).join("Steam").join("Terraria.lnk");
    plant_owned_shortcut(&planted);

    let actions = reconcile(&base, Organization::Flat);
    assert_single_move(&actions, &planted, &base.join("Terraria.lnk"));

    std::fs::remove_dir_all(&base).ok();
}

/// And into that layout from a flat one, so the nested case is covered in
/// both directions.
#[test]
fn switching_into_a_nested_per_platform_folder_moves_the_shortcut() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_into_nested_per_platform");
    std::fs::create_dir_all(&base).unwrap();

    let planted = base.join("Terraria.lnk");
    plant_owned_shortcut(&planted);

    let actions = reconcile(
        &base,
        Organization::PerPlatform {
            parent: Some(FOLDER.into()),
        },
    );
    assert_single_move(
        &actions,
        &planted,
        &base.join(FOLDER).join("Steam").join("Terraria.lnk"),
    );

    std::fs::remove_dir_all(&base).ok();
}

/// Widening the scan must not widen what is *claimable*. Hand-made shortcuts
/// sitting in directories the widened scan now visits — the base, an
/// unrelated folder, and a nested one — must all stay invisible:
/// `scan_owned` gates on the signature regardless of which directory a file
/// sits in, which is the argument that makes scanning the disk safe. Without
/// that gate, each of these would become an owned shortcut with no matching
/// desired entry, and `plan()` would emit a `DeleteOrphan` for it.
#[test]
fn scanning_the_filesystem_does_not_make_an_unsigned_shortcut_eligible() {
    let _com = ComGuard::new().unwrap();
    let base = common::unique_temp_dir("mode_switch_unsigned_stays_foreign");
    std::fs::create_dir_all(&base).unwrap();

    let foreign = [
        base.join("My Notes.lnk"),
        base.join("Work").join("Report.lnk"),
        base.join("Work").join("Steam").join("Nested.lnk"),
    ];
    for path in &foreign {
        write_lnk(
            path,
            &LnkData {
                target: system_exe(),
                args: String::new(),
                working_dir: None,
                icon: None,
                description: "My own shortcut".into(),
            },
        )
        .unwrap();
    }

    let dirs = ops::scan_dirs(&base);
    for parent in foreign.iter().filter_map(|p| p.parent()) {
        assert!(
            dirs.contains(&parent.to_path_buf()),
            "the scan must actually visit {parent:?}, or this test proves nothing: {dirs:?}"
        );
    }

    let owned = ops::scan_owned(&dirs);
    assert!(
        owned.is_empty(),
        "an unsigned shortcut must never be claimed, however many directories are scanned"
    );
    assert!(plan(&[], &owned).is_empty());

    std::fs::remove_dir_all(&base).ok();
}
