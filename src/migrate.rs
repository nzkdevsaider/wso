//! The `migrate` command: replaces the legacy `.url` shortcuts Steam
//! creates directly (a web bookmark pointing at a `steam://` URI, which
//! Windows shows with a blank generic icon — the exact complaint this whole
//! tool exists to fix) with real `.lnk` shortcuts carrying a working icon.
//!
//! Kept as its own module, separate from `commands`, specifically so the
//! orchestration/presentation code already living in `commands.rs` does not
//! grow any further; `commands::migrate` is a one-line dispatch into
//! [`run`] below.
//!
//! # Correction 1: `IconFile=` is a hint, not a resolved source
//!
//! The task brief's original premise was that a legacy `.url`'s
//! `IconFile=` line is an icon Steam already resolved, so migration should
//! prefer it outright. Checking the maintainer's real Desktop disproved
//! that: all eight real Steam `.url` shortcuts there carry an `IconFile=`
//! pointing at a hash-named `.ico` under `Steam\steam\games\`, and every
//! one of those eight files is missing from disk — Steam's own cache
//! contains exactly one `.ico`, and it matches none of them. That is the
//! actual mechanism behind the blank-icon complaint: Steam writes an icon
//! path it never populates.
//!
//! So [`icon_for`] only trusts `icon_file` when the path it names still
//! exists (`Path::is_file`); otherwise it falls through to the same
//! candidate chain `sync` already uses (`icon::resolve` over the game's
//! `icon_candidates` — executable, then library artwork, then the Steam
//! fallback). On the real machine this checked against, the fallback path
//! is exercised for all eight shortcuts, so it is the common case here, not
//! a rare exception.

use crate::cli::Cli;
use crate::commands::{confirm, exit_code};
use crate::config;
use crate::icon;
use crate::layout::{self, Target};
use crate::platform::steam::SteamProvider;
use crate::platform::{Game, GameProvider, Platform};
use crate::shortcut::signature::{decode, encode, Signature};
use crate::shortcut::url_legacy::{self, LegacyUrl};
use crate::shortcut::writer::{read_lnk, write_lnk};
use crate::shortcut::LnkData;
use std::path::{Path, PathBuf};

/// What `write_lnk`'s destination path (a `.lnk` derived from a legacy
/// `.url`'s file stem) already holds, and what that means for whether it is
/// safe to overwrite. Named Risk 5: converting `Terraria.url` produces
/// `Terraria.lnk`, and if a previous `sync` (or an earlier `migrate` run)
/// already left something there, overwriting it must be a deliberate
/// decision, never a silent one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Collision {
    /// Nothing sits at the destination path yet.
    Clear,
    /// The destination already carries our signature for this exact game —
    /// most likely a leftover from a previous `sync` or `migrate`. Safe to
    /// overwrite: this run would write an equivalent shortcut anyway.
    OwnedBySame,
    /// The destination exists and must not be touched: either it carries no
    /// `[wso:...]` ownership marker at all (hand-made, or from some other
    /// tool) or it is signed for a different game entirely. `String`
    /// explains which, for the message shown to the user.
    Blocked(String),
}

/// The pure decision behind [`Collision`], kept independent of any
/// filesystem or COM access so it is unit-testable without a real `.lnk`
/// file: given what the destination's description field already contains
/// (`None` when nothing is there yet), decide whether writing over it is
/// safe.
fn classify_existing(existing_description: Option<&str>, expected: &Signature) -> Collision {
    let Some(desc) = existing_description else {
        return Collision::Clear;
    };
    match decode(desc) {
        Some(sig) if sig == *expected => Collision::OwnedBySame,
        Some(sig) => Collision::Blocked(format!(
            "already contains a shortcut for a different game ([wso:{}:{}])",
            sig.platform.as_str(),
            sig.id
        )),
        None => Collision::Blocked("already exists and was not created by this tool".to_string()),
    }
}

/// Reads whatever currently sits at `dest`, if anything, and classifies it.
/// A file that exists but cannot be read as a shell link at all (foreign
/// data, or genuinely corrupt) is treated as blocked rather than clear —
/// the safe direction, since this function must never claim something is
/// safe to overwrite when it could not actually verify that.
fn check_existing(dest: &Path, expected: &Signature) -> Collision {
    if !dest.is_file() {
        return Collision::Clear;
    }
    match read_lnk(dest) {
        Ok(data) => classify_existing(Some(&data.description), expected),
        Err(_) => Collision::Blocked("already exists but could not be read".to_string()),
    }
}

/// Correction 1's decision, in code: prefer `icon_file` only once it is
/// confirmed to still exist, otherwise fall back to the same candidate
/// chain `sync` uses for this game (if the game was found installed at
/// all).
fn icon_for(legacy: &LegacyUrl, game: Option<&Game>, cache: &Path) -> Option<icon::ResolvedIcon> {
    legacy
        .icon_file
        .as_ref()
        .filter(|p| p.is_file())
        .map(|p| icon::ResolvedIcon { path: p.clone(), index: 0 })
        .or_else(|| {
            game.and_then(|g| {
                icon::resolve(&g.icon_candidates, cache, &format!("steam_{}", legacy.appid))
            })
        })
}

/// Every directory `migrate` scans for legacy `.url` shortcuts: wherever
/// Steam itself put them, which is the Desktop and Start Menu known
/// folders directly — not any folder this tool manages. A target that
/// cannot be resolved is simply not scanned; `migrate` has no per-item
/// failure to attach that to (there is nothing found there to report on),
/// so this mirrors `resolve_bases` in spirit but does not need its
/// bookkeeping.
fn scan_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for target in [Target::Desktop, Target::StartMenu] {
        if let Ok(dir) = layout::target_dir(target) {
            roots.push(dir);
        }
    }
    roots
}

/// Converts one legacy `.url` into a `.lnk`, or reports why it could not.
/// Returns `1` if this item failed or was blocked, `0` on success — the
/// caller sums these into the overall failed-item count.
///
/// Named Risk 1 (never destroy a shortcut that could not be replaced) is
/// enforced by construction here: `legacy.path` is only ever removed after
/// `write_lnk` has already returned `Ok`.
///
/// `pub` rather than private: this is the unit of work integration tests
/// exercise directly (`tests/migrate_url.rs`), the same way `ops::apply` is
/// a `pub` building block `tests/ops_apply.rs` calls without going through
/// a whole command's `run`. `run` below has no seams to inject a fake Steam
/// installation or fake Desktop/Start Menu directories, so testing the
/// actual collision-handling and never-destroy-on-failure guarantees has to
/// happen at this level instead.
pub fn convert_one(
    legacy: &LegacyUrl,
    steam: &SteamProvider,
    games: &[Game],
    cache: &Path,
    dry_run: bool,
) -> usize {
    let name = legacy
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Game")
        .to_string();

    let sig = Signature { platform: Platform::Steam, id: legacy.appid.clone() };
    let game = games.iter().find(|g| g.id == legacy.appid);
    let dest = legacy.path.with_extension("lnk");

    if let Collision::Blocked(reason) = check_existing(&dest, &sig) {
        if dry_run {
            println!("  would skip {name}: {} {reason}", dest.display());
        } else {
            eprintln!("  skipped {name}: {} {reason}", dest.display());
        }
        return 1;
    }

    if dry_run {
        println!("  would convert {name} -> {}", dest.display());
        return 0;
    }

    let resolved = icon_for(legacy, game, cache);
    let data = LnkData {
        target: steam.root().join("steam.exe"),
        args: format!("-applaunch {}", legacy.appid),
        working_dir: Some(steam.root().to_path_buf()),
        icon: resolved.map(|r| (r.path, r.index)),
        description: encode(game.map(|g| g.name.as_str()).unwrap_or(&name), &sig),
    };

    match write_lnk(&dest, &data) {
        Ok(()) => match std::fs::remove_file(&legacy.path) {
            Ok(()) => {
                println!("  converted {name}");
                0
            }
            Err(e) => {
                eprintln!("  converted {name}, but could not remove the .url: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("  failed to convert {name}: {e}");
            1
        }
    }
}

pub fn run(cli: &Cli) -> i32 {
    let Some(steam) = SteamProvider::detect() else {
        eprintln!("Steam is not installed; nothing to migrate.");
        return 2;
    };

    let found = url_legacy::find(&scan_roots());
    if found.is_empty() {
        println!("No Steam .url shortcuts found on the Desktop or in the Start Menu.");
        return 0;
    }

    println!("Found {} Steam .url shortcut(s):", found.len());
    for f in &found {
        println!("  {}", f.path.display());
    }
    println!("\nEach will be replaced by a .lnk with a working icon.");

    if !cli.dry_run && !confirm("Convert them now?", cli.yes) {
        println!("Cancelled.");
        return 0;
    }

    let games = steam.games().unwrap_or_default();
    let cache = config::cache_dir();
    let mut failed = 0;

    for legacy in &found {
        failed += convert_one(legacy, &steam, &games, &cache, cli.dry_run);
    }

    exit_code(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    fn sig(id: &str) -> Signature {
        Signature { platform: Platform::Steam, id: id.into() }
    }

    /// Nothing at `dest` yet: the ordinary case, always safe to proceed.
    #[test]
    fn classify_existing_is_clear_when_nothing_is_there() {
        assert_eq!(classify_existing(None, &sig("105600")), Collision::Clear);
    }

    /// Named Risk 5's safe branch: a previous `sync` (or an earlier
    /// `migrate` run) already left a `.lnk` signed for this *same* game.
    /// Overwriting it is not "silently overwriting" — the file this run
    /// would write is functionally the same shortcut, just refreshed.
    #[test]
    fn classify_existing_allows_overwriting_a_shortcut_owned_for_the_same_game() {
        let existing = crate::shortcut::signature::encode("Terraria", &sig("105600"));
        assert_eq!(
            classify_existing(Some(&existing), &sig("105600")),
            Collision::OwnedBySame
        );
    }

    /// Named Risk 5's blocked branch: a `.lnk` signed for a *different*
    /// game must never be silently clobbered.
    #[test]
    fn classify_existing_blocks_a_shortcut_owned_for_a_different_game() {
        let existing = crate::shortcut::signature::encode("Some Other Game", &sig("999"));
        let result = classify_existing(Some(&existing), &sig("105600"));
        assert!(matches!(result, Collision::Blocked(_)), "got {result:?}");
    }

    /// Named Risk 5's other blocked branch: a foreign shortcut (hand-made,
    /// or from another tool entirely, carrying no `[wso:...]` marker at
    /// all) must never be treated as safe to overwrite.
    #[test]
    fn classify_existing_blocks_a_shortcut_with_no_ownership_marker() {
        let result = classify_existing(Some("My own shortcut"), &sig("105600"));
        assert!(matches!(result, Collision::Blocked(_)), "got {result:?}");
    }
}
