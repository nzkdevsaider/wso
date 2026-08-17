//! The `migrate` command: replaces the legacy `.url` shortcuts Steam
//! creates directly (a web bookmark pointing at a `steam://` URI, which
//! Windows shows with a blank generic icon — the exact complaint this whole
//! tool exists to fix) with real `.lnk` shortcuts, with an icon attached
//! wherever one can actually be resolved.
//!
//! That last qualifier is deliberate, not decoration: this command must
//! never claim an icon is guaranteed when it cannot verify that. If
//! `SteamProvider::games` fails (a corrupt `libraryfolders.vdf` is not
//! exotic), every legacy item loses its game lookup, so [`icon_for`] has
//! nothing to fall back to once a missing `IconFile=` is rejected. Silently
//! proceeding in that state would mean deleting the original `.url` files
//! and writing icon-less `.lnk` shortcuts — this tool reproducing the exact
//! blank-icon defect it exists to fix, while telling the user it did the
//! opposite. So `run` below warns explicitly when the game lookup fails,
//! the up-front message never promises a working icon unconditionally, and
//! the summary counts and reports how many converted shortcuts actually
//! ended up with no icon.
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
use crate::shortcut::collision::{check_existing, Collision};
use crate::shortcut::signature::{encode, Signature};
use crate::shortcut::url_legacy::{self, LegacyUrl};
use crate::shortcut::writer::write_lnk;
use crate::shortcut::LnkData;
use std::path::{Path, PathBuf};

/// Correction 1's decision, in code: prefer `icon_file` only once it is
/// confirmed to still exist, otherwise fall back to the same candidate
/// chain `sync` uses for this game (if the game was found installed at
/// all).
fn icon_for(legacy: &LegacyUrl, game: Option<&Game>, cache: &Path) -> Option<icon::ResolvedIcon> {
    legacy
        .icon_file
        .as_ref()
        .filter(|p| p.is_file())
        .map(|p| icon::ResolvedIcon {
            path: p.clone(),
            index: 0,
        })
        .or_else(|| {
            game.and_then(|g| {
                icon::resolve(
                    &g.icon_candidates,
                    cache,
                    &format!("steam_{}", legacy.appid),
                )
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

/// What came of attempting one legacy `.url`'s conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvertOutcome {
    /// `1` if this item failed (a blocked collision, a write failure, or a
    /// successful write whose `.url` could then not be removed) or `0`
    /// otherwise — the caller sums these into the overall failed-item
    /// count.
    pub failed: usize,
    /// Whether the shortcut written for this item carries an icon. `Some`
    /// exactly when `write_lnk` actually succeeded (there is a `.lnk` on
    /// disk to ask the question about) — `true` if an icon was resolved for
    /// it, `false` if it was written with none. `None` when no `.lnk` was
    /// written at all: a blocked collision, a dry run, or a write failure.
    pub has_icon: Option<bool>,
}

/// Converts one legacy `.url` into a `.lnk`, or reports why it could not.
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
/// actual collision-handling, never-destroy-on-failure, and icon-honesty
/// guarantees has to happen at this level instead.
pub fn convert_one(
    legacy: &LegacyUrl,
    steam: &SteamProvider,
    games: &[Game],
    cache: &Path,
    dry_run: bool,
) -> ConvertOutcome {
    let name = legacy
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Game")
        .to_string();

    let sig = Signature {
        platform: Platform::Steam,
        id: legacy.appid.clone(),
    };
    let game = games.iter().find(|g| g.id == legacy.appid);
    let dest = legacy.path.with_extension("lnk");

    if let Collision::Blocked(reason) = check_existing(&dest, &sig) {
        if dry_run {
            println!("  would skip {name}: {} {reason}", dest.display());
        } else {
            eprintln!("  skipped {name}: {} {reason}", dest.display());
        }
        return ConvertOutcome {
            failed: 1,
            has_icon: None,
        };
    }

    if dry_run {
        println!("  would convert {name} -> {}", dest.display());
        return ConvertOutcome {
            failed: 0,
            has_icon: None,
        };
    }

    let resolved = icon_for(legacy, game, cache);
    let has_icon = resolved.is_some();
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
                if has_icon {
                    println!("  converted {name}");
                } else {
                    println!("  converted {name} (no icon could be resolved for it)");
                }
                ConvertOutcome {
                    failed: 0,
                    has_icon: Some(has_icon),
                }
            }
            Err(e) => {
                eprintln!("  converted {name}, but could not remove the .url: {e}");
                ConvertOutcome {
                    failed: 1,
                    has_icon: Some(has_icon),
                }
            }
        },
        Err(e) => {
            eprintln!("  failed to convert {name}: {e}");
            ConvertOutcome {
                failed: 1,
                has_icon: None,
            }
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
    println!(
        "\nEach will be replaced by a .lnk, with an icon attached wherever one can be resolved."
    );

    if !cli.dry_run && !confirm("Convert them now?", cli.yes) {
        println!("Cancelled.");
        return 0;
    }

    // A failed game lookup is not fatal to migration — the appid needed to
    // launch the game comes from the `.url` itself, not from this list, and
    // refusing over a cosmetic problem would cost the user something real
    // (their broken `.url` shortcuts stay broken) to avoid something merely
    // ugly (a shortcut with no icon). But it must never be *silent*: with no
    // games detected, `icon_for` has nothing to fall back to once a missing
    // `IconFile=` is rejected, so every item converted this run is at real
    // risk of losing its icon entirely.
    let games = match steam.games() {
        Ok(games) => games,
        Err(e) => {
            eprintln!(
                "Warning: Steam's game list could not be read ({e}); icons may be missing from \
                 shortcuts converted this run, since the fallback icon chain depends on it."
            );
            Vec::new()
        }
    };
    let cache = config::cache_dir();
    let mut failed = 0;
    let mut converted = 0;
    let mut without_icon = 0;

    for legacy in &found {
        let outcome = convert_one(legacy, &steam, &games, &cache, cli.dry_run);
        failed += outcome.failed;
        if let Some(has_icon) = outcome.has_icon {
            converted += 1;
            if !has_icon {
                without_icon += 1;
            }
        }
    }

    if without_icon > 0 {
        println!(
            "{without_icon} of {converted} converted shortcut(s) have no icon: the .url's \
             IconFile was missing and no fallback icon could be resolved for them."
        );
    }

    exit_code(failed)
}
