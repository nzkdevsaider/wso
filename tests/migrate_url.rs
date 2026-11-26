#![cfg(windows)]

mod common;

use std::path::PathBuf;
use winshortcutsorganizer::migrate::convert_one;
use winshortcutsorganizer::platform::steam::SteamProvider;
use winshortcutsorganizer::platform::{Game, IconCandidate, Launch, Platform};
use winshortcutsorganizer::shortcut::LnkData;
use winshortcutsorganizer::shortcut::signature::{Signature, decode, encode};
use winshortcutsorganizer::shortcut::url_legacy::LegacyUrl;
use winshortcutsorganizer::shortcut::writer::{ComGuard, read_lnk, write_lnk};

fn system_exe() -> PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(windir).join("explorer.exe")
}

fn legacy_url(path: PathBuf, appid: &str, icon_file: Option<PathBuf>) -> LegacyUrl {
    LegacyUrl {
        path,
        appid: appid.into(),
        icon_file,
    }
}

/// The ordinary case: no collision, no game detected, no still-existing
/// icon file. Proves the basic conversion writes a working `.lnk` (correct
/// launch args, our own ownership signature) and only then removes the
/// original `.url` — the general shape Named Risk 1 depends on.
#[test]
fn converts_a_url_with_no_collision_and_removes_the_original() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_convert_basic");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", None);

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(outcome.failed, 0);
    assert!(
        !url_path.exists(),
        "the .url must be removed after a successful conversion"
    );

    let lnk_path = dir.join("Terraria.lnk");
    assert!(lnk_path.is_file(), "the .lnk must have been written");

    let data = read_lnk(&lnk_path).unwrap();
    assert_eq!(data.args, "-applaunch 105600");
    assert_eq!(data.target, steam.root().join("steam.exe"));
    assert_eq!(
        decode(&data.description),
        Some(Signature {
            platform: Platform::Steam,
            id: "105600".into()
        })
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Correction 1's positive case: when the `.url`'s `IconFile=` path still
/// exists on disk, it is used exactly as-is rather than falling through to
/// any heuristic.
#[test]
fn uses_the_urls_icon_file_when_it_still_exists_on_disk() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_icon_present");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let icon_path = dir.join("cached.ico");
    std::fs::write(&icon_path, b"stand-in icon bytes").unwrap();

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", Some(icon_path.clone()));

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(outcome.failed, 0);
    assert_eq!(
        outcome.has_icon,
        Some(true),
        "a still-existing IconFile must count as a resolved icon"
    );
    let data = read_lnk(&dir.join("Terraria.lnk")).unwrap();
    assert_eq!(data.icon.map(|(p, _)| p), Some(icon_path));

    std::fs::remove_dir_all(&dir).ok();
}

/// Correction 1's actual finding, reproduced directly: the icon file named
/// in the `.url` is missing (as all eight real ones are on the maintainer's
/// Desktop), so migration must fall through to the game's own icon
/// candidate chain — exactly what `sync` uses — rather than writing a
/// shortcut pointing at a nonexistent icon.
#[test]
fn falls_back_to_the_games_icon_chain_when_the_urls_icon_file_is_missing() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_icon_missing_falls_back");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let fallback_icon = dir.join("fallback.exe");
    std::fs::write(&fallback_icon, b"x").unwrap();

    let game = Game {
        platform: Platform::Steam,
        id: "105600".into(),
        name: "Terraria".into(),
        install_dir: dir.join("Terraria"),
        launch: Launch {
            program: dir.join("Steam").join("steam.exe"),
            args: "-applaunch 105600".into(),
            working_dir: None,
        },
        icon_candidates: vec![IconCandidate::Fallback(fallback_icon.clone())],
    };

    let missing_icon = dir
        .join("Steam")
        .join("steam")
        .join("games")
        .join("gone.ico");
    assert!(
        !missing_icon.exists(),
        "fixture must name an icon that is genuinely absent"
    );

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", Some(missing_icon));

    let outcome = convert_one(
        &legacy,
        &steam,
        std::slice::from_ref(&game),
        &dir.join("cache"),
        false,
    );

    assert_eq!(outcome.failed, 0);
    assert_eq!(
        outcome.has_icon,
        Some(true),
        "the fallback icon must count as resolved"
    );
    let data = read_lnk(&dir.join("Terraria.lnk")).unwrap();
    assert_eq!(
        data.icon.map(|(p, _)| p),
        Some(fallback_icon),
        "a missing IconFile must fall through to the game's own candidate chain, not be used as-is"
    );
    assert!(
        data.description.starts_with("Terraria"),
        "a detected game's real name must be used over the bare filename, got {:?}",
        data.description
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The exact defect the coordinator's fix targets, reproduced directly:
/// the game lookup yields nothing at all (as it would if
/// `SteamProvider::games` failed and `run` passed through an empty list)
/// *and* the `.url`'s `IconFile` is missing (as all eight real files on the
/// maintainer's Desktop are). The conversion must still happen — refusing
/// would leave the user's broken `.url` in place for no benefit — but it
/// must come back honestly reporting `has_icon: Some(false)`, not silently
/// claiming success as if an icon had been attached.
#[test]
fn an_empty_game_list_and_a_missing_icon_file_produces_an_icon_less_lnk_and_reports_it_as_such() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_no_games_no_icon");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let missing_icon = dir
        .join("Steam")
        .join("steam")
        .join("games")
        .join("gone.ico");
    assert!(
        !missing_icon.exists(),
        "fixture must name an icon that is genuinely absent"
    );

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", Some(missing_icon));

    // The empty slice here stands in for `run`'s `Vec::new()` fallback when
    // `steam.games()` returns `Err` — this is the actual failure mode being
    // reproduced, not a convenience stand-in for "no game happened to be
    // installed".
    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(
        outcome.failed, 0,
        "the conversion itself must still succeed"
    );
    assert!(
        !url_path.exists(),
        "the .url is still replaced — refusing would help no one here"
    );

    let lnk_path = dir.join("Terraria.lnk");
    assert!(lnk_path.is_file());
    let data = read_lnk(&lnk_path).unwrap();
    assert_eq!(
        data.icon, None,
        "with no game and no IconFile, the written shortcut has no icon"
    );
    assert_eq!(
        outcome.has_icon,
        Some(false),
        "the outcome must own up to the missing icon rather than reporting success unqualified"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Named Risk 5, safe branch: a `.lnk` already sitting at the destination,
/// signed for this very game (e.g. left by a previous `sync`), is
/// deliberately overwritten rather than treated as a conflict.
#[test]
fn overwrites_an_existing_lnk_already_owned_for_the_same_game() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_collision_same_game");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let sig = Signature {
        platform: Platform::Steam,
        id: "105600".into(),
    };
    let lnk_path = dir.join("Terraria.lnk");
    write_lnk(
        &lnk_path,
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", None);

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(
        outcome.failed, 0,
        "overwriting our own shortcut for the same game must succeed"
    );
    assert!(!url_path.exists());
    let data = read_lnk(&lnk_path).unwrap();
    assert_eq!(
        data.target,
        steam.root().join("steam.exe"),
        "the file must actually have been rewritten, not left as the pre-existing one"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Named Risk 5, blocked branch: a `.lnk` already at the destination that
/// carries no `[wso:...]` marker at all (hand-made, or from something else
/// entirely) must be left completely untouched, and the `.url` must survive
/// since nothing was actually converted.
#[test]
fn refuses_to_overwrite_a_foreign_lnk_and_leaves_everything_untouched() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_collision_foreign");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let lnk_path = dir.join("Terraria.lnk");
    write_lnk(
        &lnk_path,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "My own shortcut".into(),
        },
    )
    .unwrap();
    let original_bytes = std::fs::read(&lnk_path).unwrap();

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", None);

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(
        outcome.failed, 1,
        "a blocked collision must count as a failed item"
    );
    assert_eq!(
        outcome.has_icon, None,
        "nothing was written, so there is no icon to report on"
    );
    assert!(
        url_path.is_file(),
        "the .url must survive when its .lnk could not be safely written"
    );
    assert_eq!(
        std::fs::read(&lnk_path).unwrap(),
        original_bytes,
        "the foreign .lnk must be left byte-for-byte untouched"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Named Risk 5, blocked branch again but for a different game's signature
/// rather than an unsigned file — proving the check compares the whole
/// signature, not merely "is this file ours at all".
#[test]
fn refuses_to_overwrite_an_lnk_owned_for_a_different_game() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_collision_different_game");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let other_sig = Signature {
        platform: Platform::Steam,
        id: "999999".into(),
    };
    let lnk_path = dir.join("Terraria.lnk");
    write_lnk(
        &lnk_path,
        &LnkData {
            target: system_exe(),
            args: "-applaunch 999999".into(),
            working_dir: None,
            icon: None,
            description: encode("Some Other Game", &other_sig),
        },
    )
    .unwrap();

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", None);

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(outcome.failed, 1);
    assert!(url_path.is_file());
    assert_eq!(
        read_lnk(&lnk_path).unwrap().args,
        "-applaunch 999999",
        "the other game's shortcut must be untouched"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Named Risk 1, the core guarantee: if `write_lnk` itself fails, the
/// original `.url` must never be removed. The destination is pre-created as
/// a directory (not a file), which guarantees `IPersistFile::Save` fails —
/// `check_existing` treats a directory as "nothing there yet"
/// (`Path::is_file` is false for it), so this specifically exercises the
/// write-failure path, not the collision path.
#[test]
fn a_failed_write_never_removes_the_original_url() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_write_fails");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let dest_as_dir = dir.join("Terraria.lnk");
    std::fs::create_dir_all(&dest_as_dir).unwrap();

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", None);

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), false);

    assert_eq!(outcome.failed, 1);
    assert_eq!(
        outcome.has_icon, None,
        "no .lnk was written, so there is no icon to report on"
    );
    assert!(
        url_path.is_file(),
        "Named Risk 1: the .url must survive when write_lnk fails"
    );
    assert!(
        dest_as_dir.is_dir(),
        "the blocked destination is left exactly as it was"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `--dry-run` must be genuinely inert: nothing written, nothing removed.
#[test]
fn dry_run_writes_no_lnk_and_removes_no_url() {
    let _com = ComGuard::new().unwrap();
    let dir = common::unique_temp_dir("migrate_dry_run");
    std::fs::create_dir_all(&dir).unwrap();
    let steam = SteamProvider::from_root(dir.join("Steam"));

    let url_path = dir.join("Terraria.url");
    std::fs::write(&url_path, "placeholder").unwrap();
    let legacy = legacy_url(url_path.clone(), "105600", None);

    let outcome = convert_one(&legacy, &steam, &[], &dir.join("cache"), true);

    assert_eq!(outcome.failed, 0);
    assert_eq!(
        outcome.has_icon, None,
        "a dry run never resolves or reports on an icon"
    );
    assert!(url_path.is_file(), "a dry run must not remove the .url");
    assert!(
        !dir.join("Terraria.lnk").exists(),
        "a dry run must not write the .lnk"
    );

    std::fs::remove_dir_all(&dir).ok();
}
