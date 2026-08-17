//! The layer that touches the user's real filesystem. Everything before this
//! module (`plan`, `layout`, `config`, ...) is pure reasoning about data;
//! `scan_owned` and `apply` are where that reasoning turns into actual reads,
//! writes and deletions.
//!
//! The tool's core guarantee — it never deletes or modifies a `.lnk` it did
//! not create — has two halves, and for a long time only the first was
//! actually implemented.
//!
//! *Never deletes* rests on `scan_owned` claiming only shortcuts whose
//! description carries the `[wso:platform:id]` marker
//! (`shortcut::signature`). A shortcut without that marker is invisible to
//! everything downstream: `plan()` never sees it, so `apply` can never
//! delete it.
//!
//! *Never modifies* needs more than that, because a `Create` or a `Move`
//! names a destination path rather than an existing owned shortcut — and a
//! foreign file can be sitting at that path. `apply` therefore checks what
//! is already there (`shortcut::collision`) before either of those two
//! writes, and reports a blocked collision as a failed item.

use crate::config::Config;
use crate::icon;
use crate::layout;
use crate::paths;
use crate::plan::{Action, DesiredShortcut, OwnedShortcut};
use crate::platform::Game;
use crate::shortcut::collision::{self, Collision};
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
            // Windows filenames are case-insensitive, so a shortcut named
            // `Game.LNK` is exactly as real a shell link as `Game.lnk`.
            // Matching case-sensitively here would make such a shortcut
            // invisible to `scan_owned` — never claimed, so `clean` could
            // never remove it and `sync` would create a duplicate beside it.
            if !paths::has_extension(&path, "lnk") {
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

/// Every directory under `base` that could be holding a shortcut this tool
/// created: `base` itself, each of its immediate subdirectories, and — inside
/// each of those — the per-platform folders this tool names.
///
/// Derived from the filesystem, deliberately, rather than from the
/// configuration. The first attempt at fixing the mode-switch defect built
/// this list from the configured layout plus the literal default `"Games"`,
/// and it only ever worked for a folder actually called `Games`.
/// `Cli::apply_overrides` replaces `cfg.organization` wholesale when
/// `--organize flat` is passed, so a user switching away from a folder named
/// `Mis Juegos` leaves nothing behind that names it: by the time anything
/// looks at the configuration, the old folder name is simply gone. Remembering
/// names structurally cannot solve this. Looking at what is on disk can, and
/// it also covers a folder whose config entry was hand-edited, corrupted, or
/// lost.
///
/// Bounded rather than recursive. The three shapes below are exactly the ones
/// `layout::shortcut_dir` can produce, so nothing deeper needs visiting:
///
/// - `base` — the flat layout.
/// - `base\<anything>` — a unified folder, under any name the user ever chose.
/// - `base\<anything>\Steam`, `base\<anything>\Epic` — the per-platform
///   layout nested under a parent, which is the only layout that sits two
///   levels down. The second level is restricted to the platform names this
///   tool writes, so the cost stays one `read_dir` of `base` plus a couple of
///   directory checks per subdirectory, rather than a walk of the user's
///   entire Desktop or Start Menu.
///
/// Widening the scan is safe by construction, and this is the same argument
/// `clean` has always relied on: `scan_owned` claims a file only if it
/// carries the `[wso:...]` signature, regardless of which directory it sits
/// in, and every deletion and every overwrite is gated on that claim
/// (`shortcut::collision`). A directory this tool has no business in simply
/// yields nothing.
///
/// This lives here, in the I/O layer, and not in `layout`: it reads the
/// filesystem, and `layout` is pure.
pub fn scan_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![base.to_path_buf()];

    let Ok(entries) = std::fs::read_dir(base) else {
        // An unreadable or missing base is not an error here: `scan_owned`
        // skips a directory it cannot read, and the caller still gets `base`
        // itself, which is the flat layout.
        return dirs;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // `is_dir` rather than `file_type`, so a junction or symlink pointing
        // at a real folder is followed. There is no recursion here, so there
        // is no cycle to fall into.
        if !path.is_dir() {
            continue;
        }
        if !dirs.contains(&path) {
            dirs.push(path.clone());
        }
        for platform in layout::all_platforms() {
            let nested = path.join(layout::sanitize_filename(platform.display_name()));
            if nested.is_dir() && !dirs.contains(&nested) {
                dirs.push(nested);
            }
        }
    }

    dirs
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
                .map(|p| icon::ResolvedIcon {
                    path: p.clone(),
                    index: 0,
                })
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

/// Refuses to write at `dest` unless this tool can prove it owns whatever is
/// already there. `Clear` (nothing at the path) and `OwnedBySame` (a leftover
/// of ours for this very game — a stale copy from a previous run, or the new
/// file of a `Move` whose cleanup half failed) both proceed; anything else
/// becomes an `Err` the caller reports as a failed item.
///
/// This is the guarantee `README.md` states plainly — `sync` never modifies
/// or removes a file it did not create — enforced at the two places that can
/// break it. Only `Create` and `Move` need it: `Retarget` and `FixIcon` are
/// emitted by `plan()` only for a path where an *owned* shortcut with the
/// matching signature was already found, so their destination is proven ours
/// before `apply` ever sees them.
///
/// Deliberately not gated behind `dry_run`: it only reads. Skipping it on a
/// dry run would make the preview claim a create that the real run will
/// refuse.
fn check_collision(
    dest: &Path,
    expected: &crate::shortcut::signature::Signature,
) -> Result<(), String> {
    match collision::check_existing(dest, expected) {
        Collision::Clear | Collision::OwnedBySame => Ok(()),
        Collision::Blocked(reason) => Err(format!("{} {reason}", dest.display())),
    }
}

/// The signature a `Move`'s destination must already carry, if it carries
/// one at all. Unlike `Create`, `Action::Move` has no `sig` field, so it is
/// recovered from the description this run is about to write — which
/// `signature::encode` always terminates with the marker.
///
/// A description that will not decode cannot be proven to belong to any
/// game, so there is no signature to compare a colliding file against and
/// the move is refused rather than allowed through unchecked. That is the
/// safe direction, and it is unreachable for anything `build_desired`
/// produces.
fn move_signature(lnk: &LnkData) -> Result<crate::shortcut::signature::Signature, String> {
    decode(&lnk.description).ok_or_else(|| {
        "the shortcut being moved carries no [wso:...] ownership marker, so what is already at \
         the destination cannot be checked against it"
            .to_string()
    })
}

/// Applies actions one at a time. A failure on one shortcut is recorded and
/// the run continues: one locked or otherwise unwritable file must never
/// stop the rest of the batch from being processed.
///
/// `dry_run` is genuinely inert: every filesystem-*modifying* branch is
/// gated behind `run`, so a dry run never creates a directory, writes a
/// file, removes a file, or touches the icon cache (icon resolution already
/// happened earlier, in `build_desired`; `apply` itself never calls
/// `icon::resolve`). The read-only collision check above runs either way,
/// so a preview and the real run agree about what is refused.
pub fn apply(actions: &[Action], dry_run: bool) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(actions.len());

    for action in actions {
        let (label, name, result) = match action {
            Action::Create(d) => (
                "create",
                display_name(&d.path),
                check_collision(&d.path, &d.sig).and_then(|()| {
                    run(dry_run, || {
                        write_lnk(&d.path, &d.lnk).map_err(|e| e.to_string())
                    })
                }),
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
                move_signature(lnk)
                    .and_then(|sig| check_collision(to, &sig))
                    .and_then(|()| run(dry_run, || {
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
                })),
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

        outcomes.push(Outcome {
            action: label.to_string(),
            name,
            result,
        });
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

    /// Gives each test its own scratch directory under the OS temp dir,
    /// unique across concurrent `cargo test` processes and across parallel
    /// threads within one, so two tests can never collide on a fixed name.
    fn scratch_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wso_scan_dirs_{label}_{pid}_{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The whole reason this reads the disk instead of the configuration:
    /// the folder names below are arbitrary, and by the time `sync` runs with
    /// `--organize flat` there is nothing in the configuration that names
    /// either of them. A name-derived scan finds neither; this one finds both
    /// without knowing anything about them.
    ///
    /// Deliberately no folder called `Games` anywhere in this test. That name
    /// was the fallback the previous, name-derived implementation carried,
    /// and including it here would let a regression to that implementation
    /// pass.
    #[test]
    fn subdirectories_are_found_under_any_name() {
        let base = scratch_dir("any_name");
        std::fs::create_dir_all(base.join("Mis Juegos")).unwrap();
        std::fs::create_dir_all(base.join("Zzz Unrelated")).unwrap();

        let dirs = scan_dirs(&base);
        assert!(dirs.contains(&base), "the flat layout is the base itself");
        assert!(dirs.contains(&base.join("Mis Juegos")), "{dirs:?}");
        assert!(dirs.contains(&base.join("Zzz Unrelated")), "{dirs:?}");

        std::fs::remove_dir_all(&base).ok();
    }

    /// `PerPlatform { parent: Some(name) }` writes into `base\<name>\Steam`,
    /// one level deeper than every other layout. A scan bounded at "base plus
    /// its immediate subdirectories" reaches `base\<name>` and stops one
    /// directory short of the shortcut, so this second level is not optional.
    #[test]
    fn platform_folders_nested_under_a_subdirectory_are_found() {
        let base = scratch_dir("nested_platform");
        std::fs::create_dir_all(base.join("Mis Juegos").join("Steam")).unwrap();
        std::fs::create_dir_all(base.join("Mis Juegos").join("Epic")).unwrap();

        let dirs = scan_dirs(&base);
        for platform in layout::all_platforms() {
            let expected = base.join("Mis Juegos").join(platform.display_name());
            assert!(
                dirs.contains(&expected),
                "expected {expected:?} in {dirs:?}"
            );
        }

        std::fs::remove_dir_all(&base).ok();
    }

    /// The bound, which is what keeps this from becoming a walk of the user's
    /// entire Desktop. The second level is restricted to the platform folder
    /// names this tool writes, and there is no third level at all — `Steam`
    /// nested inside `Steam` is past anything `layout::shortcut_dir` can
    /// produce.
    ///
    /// Without the restriction, `Notes` would be listed; without the depth
    /// bound, so would the directory below `Steam`.
    #[test]
    fn the_scan_stops_where_this_tool_stops_writing() {
        let base = scratch_dir("bounded");
        std::fs::create_dir_all(base.join("Mis Juegos").join("Notes")).unwrap();
        std::fs::create_dir_all(base.join("Mis Juegos").join("Steam").join("Steam")).unwrap();

        let dirs = scan_dirs(&base);
        assert!(dirs.contains(&base.join("Mis Juegos").join("Steam")));
        assert!(
            !dirs.contains(&base.join("Mis Juegos").join("Notes")),
            "a second-level folder that is not a platform name must not be scanned: {dirs:?}"
        );
        assert!(
            !dirs.contains(&base.join("Mis Juegos").join("Steam").join("Steam")),
            "there is no third level to scan: {dirs:?}"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    /// Files are not directories. A `.lnk` sitting in the base — which is
    /// exactly what a flat layout looks like — must not be handed to
    /// `scan_owned` as a directory to read.
    #[test]
    fn files_are_not_listed_as_directories() {
        let base = scratch_dir("files_ignored");
        std::fs::write(base.join("Terraria.lnk"), b"x").unwrap();

        let dirs = scan_dirs(&base);
        assert_eq!(dirs, vec![base.clone()], "{dirs:?}");

        std::fs::remove_dir_all(&base).ok();
    }

    /// Reading the same directory twice would let one shortcut be claimed
    /// twice and reconciled against itself. A subdirectory that happens to be
    /// named after a platform is reachable both as an immediate child and, in
    /// principle, as a nested platform folder, so this is not hypothetical.
    #[test]
    fn no_directory_is_listed_twice() {
        let base = scratch_dir("no_duplicates");
        std::fs::create_dir_all(base.join("Steam").join("Steam")).unwrap();
        std::fs::create_dir_all(base.join("Epic")).unwrap();

        let dirs = scan_dirs(&base);
        let mut unique = dirs.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), dirs.len(), "duplicates in {dirs:?}");

        std::fs::remove_dir_all(&base).ok();
    }

    /// A base that does not exist is not an error: `scan_owned` skips a
    /// directory it cannot read, and the caller must still get the base back
    /// so the flat layout is represented rather than the list coming back
    /// empty.
    #[test]
    fn an_unreadable_base_still_yields_the_base_itself() {
        let base = scratch_dir("missing_base");
        std::fs::remove_dir_all(&base).unwrap();

        assert_eq!(scan_dirs(&base), vec![base]);
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
