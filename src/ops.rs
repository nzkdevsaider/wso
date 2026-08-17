//! The layer that touches the user's real filesystem. Everything before this
//! module (`plan`, `layout`, `config`, ...) is pure reasoning about data;
//! `scan_owned` and `apply` are where that reasoning turns into actual reads,
//! writes and deletions.
//!
//! The tool's core guarantee — it never deletes or modifies a `.lnk` it did
//! not create — rests entirely on `scan_owned` claiming only shortcuts whose
//! description carries the `[wso:platform:id]` marker (`shortcut::signature`).
//! A shortcut without that marker is invisible to everything downstream:
//! `plan()` never sees it, so `apply` can never act on it.

use crate::config::Config;
use crate::icon;
use crate::layout::{self, Organization};
use crate::plan::{Action, DesiredShortcut, OwnedShortcut};
use crate::platform::Game;
use crate::shortcut::signature::{decode, encode};
use crate::shortcut::writer::{read_lnk, write_lnk};
use crate::shortcut::LnkData;
use std::path::{Path, PathBuf};

/// Reads every `.lnk` in the given directories and keeps only the ones
/// carrying our signature. Anything unsigned — a hand-made shortcut, one
/// from another tool, a non-`.lnk` file, a directory, or a `.lnk` that
/// cannot be read at all — is skipped rather than claimed, which is what
/// makes deletion safe. A directory that cannot be read (missing, or
/// otherwise inaccessible) is skipped too, rather than aborting the whole
/// scan: one bad directory must not hide what was found in the others.
///
/// The result is sorted by path so callers see a deterministic order
/// regardless of the underlying filesystem's directory-entry order — this
/// matters most when two owned shortcuts legitimately share a signature
/// (see `plan::SkipReason::DuplicateSignature` and the note on `Action::Move`
/// in this module), where which entry `plan()` treats as "the" shortcut for
/// a game depends on the order `scan_owned` returns them in.
pub fn scan_owned(dirs: &[PathBuf]) -> Vec<OwnedShortcut> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Windows filesystems are case-insensitive, so a shortcut named
            // `Game.LNK` is exactly as real a shell link as `Game.lnk`. The
            // same `eq_ignore_ascii_case` idiom is already used for `.exe`
            // in `icon::exe_finder`; matching case-sensitively here would
            // make such a shortcut invisible to `scan_owned` — never
            // claimed, so `clean` could never remove it and `sync` would
            // create a duplicate beside it.
            let is_lnk = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
            if !is_lnk {
                continue;
            }
            if !path.is_file() {
                // Guards against a directory that happens to be named
                // `something.lnk`: never hand a directory to `read_lnk`.
                continue;
            }
            let Ok(lnk) = read_lnk(&path) else { continue };
            let Some(sig) = decode(&lnk.description) else {
                continue;
            };
            out.push(OwnedShortcut { sig, path, lnk });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Turns detected games into the shortcuts that should exist, honoring
/// platform selection, exclusions, icon overrides and the chosen
/// organization.
///
/// Two `Game` entries can legitimately share a `(platform, id)`: Steam can
/// leave a stale `appmanifest_<appid>.acf` behind in an old library after a
/// game moves drives, and the Steam provider iterates every library without
/// deduplicating, so the same appid can appear twice upstream. `plan()`
/// already defends against the symptom (`SkipReason::DuplicateSignature`),
/// but deduplicating here — keeping the first occurrence — means the
/// duplicate never becomes a desired shortcut in the first place.
pub fn build_desired(
    games: &[Game],
    cfg: &Config,
    base: &Path,
    cache: &Path,
) -> Vec<DesiredShortcut> {
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<&Game> = games
        .iter()
        .filter(|g| seen.insert((g.platform, g.id.clone())))
        .collect();

    let included: Vec<&Game> = deduped
        .into_iter()
        .filter(|g| cfg.platforms.contains(&g.platform))
        .filter(|g| !cfg.is_excluded(g.platform, &g.id, &g.name))
        .collect();

    let name_inputs: Vec<(crate::platform::Platform, String)> = included
        .iter()
        .map(|g| (g.platform, g.name.clone()))
        .collect();
    let filenames = layout::assign_filenames(&name_inputs);

    included
        .iter()
        .zip(filenames)
        .map(|(g, filename)| {
            let qualified = format!("{}:{}", g.platform.as_str(), g.id);
            let resolved = cfg
                .icon_overrides
                .get(&qualified)
                .map(|p| icon::ResolvedIcon { path: p.clone(), index: 0 })
                .or_else(|| icon::resolve(&g.icon_candidates, cache, &qualified.replace(':', "_")));

            let sig = crate::shortcut::signature::Signature {
                platform: g.platform,
                id: g.id.clone(),
            };

            DesiredShortcut {
                path: layout::shortcut_dir(base, &cfg.organization, g.platform)
                    .join(format!("{filename}.lnk")),
                lnk: LnkData {
                    target: g.launch.program.clone(),
                    args: g.launch.args.clone(),
                    working_dir: g.launch.working_dir.clone(),
                    icon: resolved.map(|r| (r.path, r.index)),
                    description: encode(&g.name, &sig),
                },
                sig,
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub action: String,
    pub name: String,
    pub result: Result<(), String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub created: usize,
    pub updated: usize,
    pub moved: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub failed: usize,
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}

/// Applies actions one at a time. A failure on one shortcut is recorded and
/// the run continues: one locked or otherwise unwritable file must never
/// stop the rest of the batch from being processed.
///
/// `dry_run` is genuinely inert: every filesystem-touching branch is gated
/// behind `run`, so a dry run never creates a directory, writes a file,
/// removes a file, or touches the icon cache (icon resolution already
/// happened earlier, in `build_desired`; `apply` itself never calls
/// `icon::resolve`).
pub fn apply(actions: &[Action], dry_run: bool) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(actions.len());

    for action in actions {
        let (label, name, result) = match action {
            Action::Create(d) => (
                "create",
                display_name(&d.path),
                run(dry_run, || write_lnk(&d.path, &d.lnk).map_err(|e| e.to_string())),
            ),
            Action::Retarget { path, lnk } => (
                "retarget",
                display_name(path),
                run(dry_run, || write_lnk(path, lnk).map_err(|e| e.to_string())),
            ),
            Action::FixIcon { path, lnk } => (
                "fix-icon",
                display_name(path),
                run(dry_run, || write_lnk(path, lnk).map_err(|e| e.to_string())),
            ),
            Action::Move { from, to, lnk } => (
                "move",
                display_name(to),
                run(dry_run, || {
                    // Write the new file first. If this fails, nothing on
                    // disk has changed yet: `from` is untouched and `to`
                    // was never created.
                    write_lnk(to, lnk).map_err(|e| e.to_string())?;

                    if from != to {
                        // The new shortcut now exists and is correct even
                        // if the next step fails, so a failure here must
                        // not undo it: deleting `to` to "roll back" would
                        // trade a recoverable leftover file for losing the
                        // shortcut entirely. Instead, report the failure —
                        // the old file is left in place as a duplicate,
                        // which a future scan/plan pass can reconcile
                        // (`plan::SkipReason::DuplicateSignature` and its
                        // orphan handling already exist to resolve two
                        // owned shortcuts sharing a signature) rather than
                        // silently reporting this move as fully done.
                        std::fs::remove_file(from).map_err(|e| {
                            format!(
                                "wrote the new shortcut but could not remove the old file at {}: {e}",
                                from.display()
                            )
                        })?;
                    }
                    Ok(())
                }),
            ),
            Action::DeleteOrphan(path) => (
                "delete",
                display_name(path),
                run(dry_run, || {
                    std::fs::remove_file(path).map_err(|e| e.to_string())
                }),
            ),
            Action::Skip { path, .. } => ("skip", display_name(path), Ok(())),
        };

        outcomes.push(Outcome { action: label.to_string(), name, result });
    }

    outcomes
}

fn run<F>(dry_run: bool, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    if dry_run {
        Ok(())
    } else {
        f()
    }
}

pub fn summarize(outcomes: &[Outcome]) -> Summary {
    let mut s = Summary::default();
    for o in outcomes {
        if o.result.is_err() {
            s.failed += 1;
            continue;
        }
        match o.action.as_str() {
            "create" => s.created += 1,
            "retarget" | "fix-icon" => s.updated += 1,
            "move" => s.moved += 1,
            "delete" => s.deleted += 1,
            _ => s.skipped += 1,
        }
    }
    s
}

/// Directories the tool may own for a given base and organization. Thin
/// re-export so callers do not need to reach into `layout` directly.
pub fn managed_dirs(base: &Path, org: &Organization) -> Vec<PathBuf> {
    layout::managed_dirs(base, org)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{IconCandidate, Launch, Platform};
    use std::path::PathBuf;

    fn game(id: &str, name: &str) -> Game {
        Game {
            platform: Platform::Steam,
            id: id.into(),
            name: name.into(),
            install_dir: PathBuf::from(r"C:\Games\Game"),
            launch: Launch {
                program: PathBuf::from(r"C:\Games\steam.exe"),
                args: format!("-applaunch {id}"),
                working_dir: None,
            },
            icon_candidates: vec![IconCandidate::Fallback(PathBuf::from(
                r"C:\Windows\explorer.exe",
            ))],
        }
    }

    /// Steam can leave a stale `appmanifest_<appid>.acf` behind in an old
    /// library after a game moves drives, and the provider iterates every
    /// library without deduplicating, so the same `(platform, id)` can
    /// legitimately appear twice among `games`. The two entries here have
    /// distinct install directories, so this proves deduplication is keyed
    /// on `(platform, id)` rather than coincidentally collapsing two
    /// identical structs.
    #[test]
    fn build_desired_deduplicates_two_games_sharing_a_platform_and_id() {
        let mut first = game("440", "Team Fortress 2");
        first.install_dir = PathBuf::from(r"D:\SteamLibrary\steamapps\common\Team Fortress 2");
        let mut second = game("440", "Team Fortress 2");
        second.install_dir = PathBuf::from(r"E:\OldSteamLibrary\steamapps\common\Team Fortress 2");

        let games = [first, second];
        let cfg = Config::default();
        let base = PathBuf::from(r"C:\Desktop");
        let cache = PathBuf::from(r"C:\cache");

        let desired = build_desired(&games, &cfg, &base, &cache);
        assert_eq!(
            desired.len(),
            1,
            "the duplicate (platform, id) must not become a second desired shortcut"
        );
        assert_eq!(desired[0].sig.id, "440");
    }

    /// Same id, different platform: the dedup key is `(platform, id)`, not
    /// `id` alone, so this must NOT be collapsed the way the test above is.
    #[test]
    fn build_desired_does_not_deduplicate_the_same_id_across_different_platforms() {
        let mut steam_game = game("440", "Some Game");
        steam_game.platform = Platform::Steam;
        let mut epic_game = game("440", "Some Game");
        epic_game.platform = Platform::Epic;

        let games = [steam_game, epic_game];
        let cfg = Config::default();
        let base = PathBuf::from(r"C:\Desktop");
        let cache = PathBuf::from(r"C:\cache");

        let desired = build_desired(&games, &cfg, &base, &cache);
        assert_eq!(
            desired.len(),
            2,
            "the same id on two different platforms must produce two desired shortcuts"
        );
    }
}
