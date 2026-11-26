//! Pure reconciliation between the shortcuts that should exist and the
//! shortcuts this tool already owns. No I/O happens here, which is what makes
//! every command's behavior testable without Windows, Steam or Epic.

use crate::shortcut::LnkData;
use crate::shortcut::signature::Signature;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredShortcut {
    pub sig: Signature,
    pub path: PathBuf,
    pub lnk: LnkData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedShortcut {
    pub sig: Signature,
    pub path: PathBuf,
    pub lnk: LnkData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    UpToDate,
    /// A later `desired` entry shares a signature with an earlier one
    /// already reconciled in this same `plan()` call. Only the first entry
    /// for a signature acts; every later one is skipped so it cannot also
    /// act on whatever owned shortcut (or lack of one) the first entry
    /// already claimed — e.g. Steam can leave a stale `appmanifest_*.acf`
    /// behind in an old library after a game moves drives, producing two
    /// `Game` entries with the same appid upstream.
    DuplicateSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create(DesiredShortcut),
    /// Rewrites the shortcut in place at its current path: the launch
    /// command, working directory, or description changed, but not the
    /// path itself (an icon-only change is a `FixIcon` instead).
    Retarget {
        path: PathBuf,
        lnk: LnkData,
    },
    FixIcon {
        path: PathBuf,
        lnk: LnkData,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        lnk: LnkData,
    },
    DeleteOrphan(PathBuf),
    Skip {
        path: PathBuf,
        reason: SkipReason,
    },
}

/// Folds a path into the form two spellings of the same Windows path agree
/// on: separators normalized to `\`, and ASCII letters lowercased.
///
/// Pure string work — no `canonicalize`, no metadata lookup, nothing that
/// touches the filesystem — because `plan()` must stay pure (spec §5), and
/// because the paths being compared can name files that do not exist on this
/// machine at all.
///
/// ASCII-only case folding, matching the `eq_ignore_ascii_case` idiom already
/// used for extensions elsewhere in the crate. Windows' own case-insensitivity
/// extends past ASCII, so this is the conservative direction: at worst two
/// spellings that Windows would consider equal are reported as different,
/// which costs a redundant rewrite, never a missed one.
fn path_key(p: &Path) -> String {
    p.to_string_lossy()
        .chars()
        .map(|c| {
            if c == '/' {
                '\\'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Whether two paths name the same thing, tolerating the two ways a shortcut's
/// stored path can differ from the one a provider hands us for reasons that
/// have nothing to do with the shortcut being wrong.
///
/// Windows resolves a shell link's local-file target and working directory to
/// the actual on-disk casing when the link is saved — that is how links stay
/// valid across a case rename — so what `read_lnk` returns need not match the
/// string that was written. `tests/lnk_roundtrip.rs` carries a `canonical()`
/// helper introduced for exactly this: a machine whose `%WINDIR%` reads
/// `C:\WINDOWS` while the disk entry is `C:\Windows`. Providers supply raw
/// paths from their own sources — `SteamProvider::detect` tries
/// `HKCU\Software\Valve\Steam\SteamPath` first, which conventionally holds a
/// lowercase forward-slash path — so the two sides can legitimately disagree
/// on case or separator while naming the same file.
///
/// Comparing them raw makes every run emit a `Retarget` for every affected
/// shortcut, forever: rewriting one that was already correct, reporting churn
/// that never converges, and burying any genuine change in the noise.
///
/// This is reproduced, not hypothetical, though not through the registry
/// route it was first suspected of. `EpicProvider::explorer()` builds its
/// launch program from `%WINDIR%`, and on the maintainer's machine `%WINDIR%`
/// reads `C:\WINDOWS` while the directory on disk is `C:\Windows`. Windows
/// stores the resolved on-disk casing when the link is saved, so every Epic
/// shortcut comes back spelled differently from the way it was asked for.
/// `tests/idempotence.rs` fails on exactly that when this comparison is
/// reverted to `!=`. A Steam-only library does not show it, because Steam's
/// root comes from a conventional path that already matches the disk — which
/// is why two consecutive `sync` runs there reported `0 changes, 9
/// unchanged`.
///
/// Deliberately not applied to `path`: where a shortcut *file* lives decides
/// `Move` and `DeleteOrphan`, and those must keep comparing exactly.
///
/// `pub(crate)` so `ops` can reuse the same fold when it decides whether a
/// `Move`'s two endpoints are really one file (`ops::names_the_same_file`).
/// One definition of "these two spellings mean the same path", not two that
/// could drift.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// [`same_path`] lifted over `Option`, for `working_dir`. `None` and `Some`
/// remain different: a shortcut that has lost its working directory is
/// genuinely out of sync.
fn same_optional_path(a: &Option<PathBuf>, b: &Option<PathBuf>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same_path(a, b),
        _ => false,
    }
}

/// Reconciles what should exist against what this tool already owns.
///
/// Matching is by signature, never by path or filename, so a shortcut the
/// user renamed or moved is still recognized as the same game.
///
/// If two `desired` entries share a signature — which can happen upstream,
/// e.g. a stale Steam library manifest producing a duplicate `Game` — only
/// the first is reconciled; every later one is skipped with
/// `SkipReason::DuplicateSignature` rather than also acting on whatever the
/// first entry already claimed.
///
/// Two `owned` entries can also legitimately share a signature — for
/// instance, the write half of a `Move` can succeed while the removal of
/// the old file fails (see `ops::apply`), leaving both the old and the new
/// `.lnk` on disk with the same signature until the next run reconciles
/// them. When that happens, the entry whose `path` already equals `d.path`
/// is preferred over the first entry by position: this is what makes
/// recovery deterministic and correct rather than an accident of whatever
/// order `scan_owned` happened to return the two files in. Without the
/// path preference, picking the *stale* entry first would emit a `Move`
/// (rewriting the already-correct new file, harmless on its own) followed
/// by a `DeleteOrphan` for the *other* entry — which is the new file,
/// deleting the shortcut the previous run had already gotten right, so the
/// shortcut disappears entirely instead of recovering. Preferring the
/// already-correctly-placed entry means it is matched and skipped, and the
/// stale one is the one left to become the orphan.
pub fn plan(desired: &[DesiredShortcut], owned: &[OwnedShortcut]) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut matched: Vec<bool> = vec![false; owned.len()];
    let mut claimed_signatures: Vec<&Signature> = Vec::new();

    for d in desired {
        if claimed_signatures.iter().any(|s| **s == d.sig) {
            actions.push(Action::Skip {
                path: d.path.clone(),
                reason: SkipReason::DuplicateSignature,
            });
            continue;
        }
        claimed_signatures.push(&d.sig);

        let existing = owned
            .iter()
            .position(|o| o.sig == d.sig && o.path == d.path)
            .or_else(|| owned.iter().position(|o| o.sig == d.sig));

        let Some(idx) = existing else {
            actions.push(Action::Create(d.clone()));
            continue;
        };
        matched[idx] = true;
        let o = &owned[idx];

        // A relocation rewrites the file at the new path, so it subsumes any
        // launch-command, description, or icon change and must be checked
        // first.
        if o.path != d.path {
            actions.push(Action::Move {
                from: o.path.clone(),
                to: d.path.clone(),
                lnk: d.lnk.clone(),
            });
        } else if !same_path(&o.lnk.target, &d.lnk.target)
            || o.lnk.args != d.lnk.args
            || !same_optional_path(&o.lnk.working_dir, &d.lnk.working_dir)
            || o.lnk.description != d.lnk.description
        {
            // `description` is grouped with the launch-command fields, not
            // with the icon: both sides are produced by the same
            // deterministic `signature::encode`, so it changes exactly when
            // the underlying game title changes — and `sanitize_filename`
            // is many-to-one (e.g. "Trailmakers." and "Trailmakers" both
            // sanitize to "Trailmakers"), so a title change is not
            // guaranteed to also change `path` and trigger a `Move`.
            // Without this, such a title change would be invisible here and
            // the shortcut's tooltip would never refresh.
            actions.push(Action::Retarget {
                path: d.path.clone(),
                lnk: d.lnk.clone(),
            });
        } else if o.lnk.icon != d.lnk.icon {
            actions.push(Action::FixIcon {
                path: d.path.clone(),
                lnk: d.lnk.clone(),
            });
        } else {
            actions.push(Action::Skip {
                path: d.path.clone(),
                reason: SkipReason::UpToDate,
            });
        }
    }

    for (idx, o) in owned.iter().enumerate() {
        if !matched[idx] {
            actions.push(Action::DeleteOrphan(o.path.clone()));
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use std::path::PathBuf;

    fn sig(id: &str) -> Signature {
        Signature {
            platform: Platform::Steam,
            id: id.into(),
        }
    }

    fn lnk(target: &str, args: &str, icon: &str) -> LnkData {
        LnkData {
            target: PathBuf::from(target),
            args: args.into(),
            working_dir: None,
            icon: Some((PathBuf::from(icon), 0)),
            description: "d".into(),
        }
    }

    fn lnk_with_description(target: &str, args: &str, icon: &str, description: &str) -> LnkData {
        LnkData {
            description: description.into(),
            ..lnk(target, args, icon)
        }
    }

    fn desired(id: &str, path: &str, icon: &str) -> DesiredShortcut {
        DesiredShortcut {
            sig: sig(id),
            path: PathBuf::from(path),
            lnk: lnk("steam.exe", &format!("-applaunch {id}"), icon),
        }
    }

    fn owned(id: &str, path: &str, icon: &str) -> OwnedShortcut {
        OwnedShortcut {
            sig: sig(id),
            path: PathBuf::from(path),
            lnk: lnk("steam.exe", &format!("-applaunch {id}"), icon),
        }
    }

    #[test]
    fn creates_a_missing_shortcut() {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let actions = plan(&d, &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Create(_)));
    }

    #[test]
    fn skips_an_up_to_date_shortcut() {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(
            plan(&d, &o).as_slice(),
            [Action::Skip {
                reason: SkipReason::UpToDate,
                ..
            }]
        ));
    }

    // `is_idempotent` used to live here: it called a pure function twice
    // with the same arguments and asserted the two results had equal
    // lengths, which no change to `plan()` could ever make fail. The real
    // property spec §12 asks for — write a shortcut, read it back off disk,
    // re-plan, and get nothing but `Skip` — cannot be checked without a
    // filesystem round trip, so it lives in `tests/idempotence.rs` instead.

    /// Windows resolves a shell link's local-file target to the actual
    /// on-disk casing when it is saved, so `read_lnk` can hand back
    /// `C:\WINDOWS\explorer.exe` where the provider supplied
    /// `C:\Windows\explorer.exe`. Compared raw, that is a `Retarget` — on
    /// every run, forever, rewriting a shortcut that was already correct.
    /// Nothing but case differs here, and `path` and `icon` are identical, so
    /// only case-insensitive comparison of `target` can produce a `Skip`.
    #[test]
    fn a_target_differing_only_in_letter_case_is_not_a_change() {
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"C:\Windows\explorer.exe", "-applaunch 440", "tf2.exe"),
        }];
        let o = [OwnedShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"C:\WINDOWS\explorer.exe", "-applaunch 440", "tf2.exe"),
        }];
        assert!(matches!(
            plan(&d, &o).as_slice(),
            [Action::Skip {
                reason: SkipReason::UpToDate,
                ..
            }]
        ));
    }

    /// The other spelling difference the same paths can pick up:
    /// `SteamProvider::detect` reads `HKCU\Software\Valve\Steam\SteamPath`
    /// first, which conventionally holds a forward-slash path, while a saved
    /// shell link stores backslashes. Both spellings name the same file.
    #[test]
    fn a_target_differing_only_in_path_separator_is_not_a_change() {
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(
                "c:/program files (x86)/steam/steam.exe",
                "-applaunch 440",
                "tf2.exe",
            ),
        }];
        let o = [OwnedShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(
                r"C:\Program Files (x86)\Steam\steam.exe",
                "-applaunch 440",
                "tf2.exe",
            ),
        }];
        assert!(matches!(
            plan(&d, &o).as_slice(),
            [Action::Skip {
                reason: SkipReason::UpToDate,
                ..
            }]
        ));
    }

    /// The guard on the two tests above: the comparison must still be a
    /// comparison. These two targets are different files, not two spellings
    /// of one, and a helper that folded everything together — or was wired up
    /// to always return `true` — would report `Skip` here too.
    #[test]
    fn a_genuinely_different_target_is_still_a_retarget() {
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"D:\Steam\steam.exe", "-applaunch 440", "tf2.exe"),
        }];
        let o = [OwnedShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"C:\Steam\steam.exe", "-applaunch 440", "tf2.exe"),
        }];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Retarget { .. }]));
    }

    /// `working_dir` is saved and resolved by Windows the same way `target`
    /// is, so it needs the same tolerance. `target` is identical here, which
    /// means only the `working_dir` comparison can decide the outcome.
    #[test]
    fn a_working_directory_differing_only_in_case_is_not_a_change() {
        let same = lnk(r"C:\Steam\steam.exe", "-applaunch 440", "tf2.exe");
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: LnkData {
                working_dir: Some(PathBuf::from(r"C:\Steam")),
                ..same.clone()
            },
        }];
        let o = [OwnedShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: LnkData {
                working_dir: Some(PathBuf::from(r"c:\steam")),
                ..same
            },
        }];
        assert!(matches!(
            plan(&d, &o).as_slice(),
            [Action::Skip {
                reason: SkipReason::UpToDate,
                ..
            }]
        ));
    }

    /// A shortcut that has lost its working directory entirely is genuinely
    /// out of sync — `None` must never fold together with `Some`, however
    /// forgiving the comparison of two `Some`s is.
    #[test]
    fn a_working_directory_that_disappeared_is_still_a_retarget() {
        let same = lnk(r"C:\Steam\steam.exe", "-applaunch 440", "tf2.exe");
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: LnkData {
                working_dir: Some(PathBuf::from(r"C:\Steam")),
                ..same.clone()
            },
        }];
        let o = [OwnedShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: LnkData {
                working_dir: None,
                ..same
            },
        }];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Retarget { .. }]));
    }

    /// Where a shortcut *file* lives decides `Move` and `DeleteOrphan`, so
    /// `path` keeps comparing exactly. Two directories differing only in case
    /// would be the same directory on Windows, but folding them here would
    /// mean `plan()` silently deciding not to relocate a shortcut the user
    /// asked to relocate — and the same fold applied to `DeleteOrphan` would
    /// change which file gets deleted. Guessing wrong about a deletion is the
    /// expensive mistake, so `plan()` stays literal and leaves the question to
    /// the layer that can actually ask the filesystem.
    ///
    /// This test used to claim, flatly, that "a redundant `Move` is cheap".
    /// It was not: `ops::apply` guarded its cleanup step with a byte-exact
    /// `from != to`, so a `Move` between two spellings of one file overwrote
    /// that file and then deleted it, reporting success. The claim is true
    /// only because `ops::names_the_same_file` now makes it true — which is
    /// where a redundant `Move` is made cheap, not here.
    #[test]
    fn a_destination_path_differing_only_in_case_is_still_a_move() {
        let d = [desired("440", r"C:\Desktop\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\DESKTOP\TF2.lnk", "tf2.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn fixes_a_changed_icon() {
        let d = [desired("440", r"C:\D\TF2.lnk", "new.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "old.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::FixIcon { .. }]));
    }

    #[test]
    fn retargets_when_the_launch_command_changed() {
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"D:\Steam\steam.exe", "-applaunch 440", "tf2.exe"),
        }];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Retarget { .. }]));
    }

    #[test]
    fn moves_when_the_organization_mode_changed() {
        let d = [desired("440", r"C:\D\Games\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        match plan(&d, &o).as_slice() {
            [Action::Move { from, to, .. }] => {
                assert_eq!(from, &PathBuf::from(r"C:\D\TF2.lnk"));
                assert_eq!(to, &PathBuf::from(r"C:\D\Games\TF2.lnk"));
            }
            other => panic!("expected a Move, got {other:?}"),
        }
    }

    #[test]
    fn a_move_wins_over_an_icon_change() {
        // A relocated shortcut is rewritten wholesale at the new path, so
        // there is no separate FixIcon to emit.
        let d = [desired("440", r"C:\D\Games\TF2.lnk", "new.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "old.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn deletes_an_orphan_when_the_game_is_gone() {
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(
            plan(&[], &o).as_slice(),
            [Action::DeleteOrphan(_)]
        ));
    }

    #[test]
    fn clean_deletes_everything_owned() {
        let o = [
            owned("440", r"C:\D\TF2.lnk", "a.exe"),
            owned("570", r"C:\D\Dota.lnk", "b.exe"),
        ];
        let actions = plan(&[], &o);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|a| matches!(a, Action::DeleteOrphan(_))));
    }

    #[test]
    fn matches_on_signature_not_on_path() {
        // The same game, renamed on disk by the user, is still the same game.
        let d = [desired("440", r"C:\D\Team Fortress 2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn different_platforms_with_the_same_id_do_not_match() {
        let d = [DesiredShortcut {
            sig: Signature {
                platform: Platform::Epic,
                id: "440".into(),
            },
            path: PathBuf::from(r"C:\D\E.lnk"),
            lnk: lnk("explorer.exe", "uri", "e.exe"),
        }];
        let o = [owned("440", r"C:\D\S.lnk", "s.exe")];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().any(|a| matches!(a, Action::Create(_))));
        assert!(actions.iter().any(|a| matches!(a, Action::DeleteOrphan(_))));
    }

    #[test]
    fn handles_a_mixed_batch() {
        let d = [
            desired("440", r"C:\D\TF2.lnk", "tf2.exe"),  // up to date
            desired("570", r"C:\D\Dota.lnk", "new.exe"), // icon changed
            desired("620", r"C:\D\P2.lnk", "p2.exe"),    // new
        ];
        let o = [
            owned("440", r"C:\D\TF2.lnk", "tf2.exe"),
            owned("570", r"C:\D\Dota.lnk", "old.exe"),
            owned("999", r"C:\D\Gone.lnk", "gone.exe"), // uninstalled
        ];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 4);
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::Skip { .. }))
                .count(),
            1
        );
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::FixIcon { .. }))
                .count(),
            1
        );
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::Create(_)))
                .count(),
            1
        );
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::DeleteOrphan(_)))
                .count(),
            1
        );
    }

    // --- Gaps found while reviewing the brief's test list ---

    #[test]
    fn plan_of_nothing_desired_and_nothing_owned_produces_no_actions() {
        // Neither a Create (nothing desired) nor a DeleteOrphan (nothing
        // owned) has any input to react to. `status` on a machine with no
        // selected games and no existing shortcuts must print nothing, not
        // merely "not fail".
        let actions: Vec<Action> = plan(&[], &[]);
        assert!(actions.is_empty());
    }

    #[test]
    fn a_retarget_subsumes_a_simultaneous_icon_change() {
        // The brief's own `retargets_when_the_launch_command_changed` test
        // only varies the target, leaving the icon identical between
        // `desired` and `owned` — so it cannot distinguish "Retarget beats
        // FixIcon" from "FixIcon was never a candidate". Here both the
        // target and the icon differ at the same path, so only the
        // precedence check (target/args before icon) can produce a single
        // Retarget instead of a Retarget-then-FixIcon pair or a bare
        // FixIcon.
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"D:\Steam\steam.exe", "-applaunch 440", "new.exe"),
        }];
        let o = [owned("440", r"C:\D\TF2.lnk", "old.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Retarget { .. }]));
    }

    #[test]
    fn a_move_subsumes_a_simultaneous_retarget_and_icon_change() {
        // The brief's `a_move_wins_over_an_icon_change` only varies path and
        // icon, leaving args/target identical — so it cannot rule out that a
        // path change plus a launch-command change would emit both a Move
        // and a Retarget. Here path, target/args, and icon all differ at
        // once, and only a single Move may come out: proof that a
        // relocation truly subsumes every other kind of change, not just an
        // icon change.
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\Games\TF2.lnk"),
            lnk: lnk(r"D:\Steam\steam.exe", "-applaunch 440", "new.exe"),
        }];
        let o = [owned("440", r"C:\D\TF2.lnk", "old.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn a_duplicate_owned_signature_leaves_only_the_first_match_and_deletes_the_rest() {
        // Two owned shortcuts sharing a signature is not something this
        // layer can prevent (a user could copy a .lnk file by hand). Only
        // the matching-by-position mechanism decides which survives: the
        // first owned entry with that signature is reconciled against
        // `desired`, and every other owned entry with the same signature
        // is treated as an orphan, since it cannot also be "the" shortcut
        // for that game.
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let o = [
            owned("440", r"C:\D\TF2.lnk", "tf2.exe"),
            owned("440", r"C:\D\TF2 - Copy.lnk", "tf2.exe"),
        ];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 2);
        assert!(matches!(
            actions[0],
            Action::Skip {
                reason: SkipReason::UpToDate,
                ..
            }
        ));
        assert!(matches!(
            &actions[1],
            Action::DeleteOrphan(p) if p == &PathBuf::from(r"C:\D\TF2 - Copy.lnk")
        ));
    }

    /// The test above happens to place the correctly-located owned entry
    /// first, so it cannot distinguish "matches by path" from "matches by
    /// position and got lucky." This reaches the same duplicate-signature
    /// situation a partially-failed `Move` produces (see `ops::apply`'s
    /// `Move` handling): a stale copy left at the old path, and a correct
    /// copy already sitting at the desired path. Supplying the two owned
    /// entries in both orders proves the outcome — the correctly-placed
    /// entry is matched and skipped, the stale one becomes the orphan — is
    /// independent of scan order, not an accident of whichever order
    /// `scan_owned` happened to return them in.
    #[test]
    fn a_duplicate_owned_signature_prefers_the_entry_at_the_desired_path_regardless_of_scan_order()
    {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let stale = owned("440", r"C:\D\Old\TF2.lnk", "tf2.exe");
        let correct = owned("440", r"C:\D\TF2.lnk", "tf2.exe");

        for o in [
            vec![stale.clone(), correct.clone()],
            vec![correct.clone(), stale.clone()],
        ] {
            let actions = plan(&d, &o);
            assert_eq!(actions.len(), 2);

            let skip = actions
                .iter()
                .find(|a| matches!(a, Action::Skip { .. }))
                .unwrap_or_else(|| panic!("expected a Skip among {actions:?}"));
            assert!(matches!(
                skip,
                Action::Skip {
                    reason: SkipReason::UpToDate,
                    ..
                }
            ));

            let orphan = actions
                .iter()
                .find(|a| matches!(a, Action::DeleteOrphan(_)))
                .unwrap_or_else(|| panic!("expected a DeleteOrphan among {actions:?}"));
            assert!(matches!(
                orphan,
                Action::DeleteOrphan(p) if p == &PathBuf::from(r"C:\D\Old\TF2.lnk")
            ));
        }
    }

    #[test]
    fn a_duplicate_desired_signature_only_acts_once_and_flags_the_rest() {
        // Two desired entries sharing a signature is reachable in practice:
        // Steam can leave a stale appmanifest behind in an old library
        // after a game moves to another drive, producing two `Game`
        // entries with the same appid upstream. Without deduplication
        // here, both would resolve to the same owned index and both would
        // act on it — producing two Move actions with the same `from`,
        // the second acting on a file the first already relocated. Only
        // the first desired entry for a signature may act; the rest must
        // be skipped with a reason that names the cause, not silently
        // dropped and not silently duplicated.
        let d = [
            desired("440", r"C:\D\Games\TF2.lnk", "tf2.exe"),
            desired("440", r"C:\D\Games\TF2 Again.lnk", "tf2.exe"),
        ];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0], Action::Move { .. }));
        match &actions[1] {
            Action::Skip {
                path,
                reason: SkipReason::DuplicateSignature,
            } => {
                assert_eq!(path, &PathBuf::from(r"C:\D\Games\TF2 Again.lnk"));
            }
            other => panic!("expected a duplicate-signature Skip, got {other:?}"),
        }
    }

    #[test]
    fn a_retarget_is_emitted_when_only_the_description_changed() {
        // sanitize_filename (src/layout.rs) is many-to-one: "Trailmakers."
        // and "Trailmakers" both sanitize to "Trailmakers". So a title
        // change confined to punctuation stripped by sanitization leaves
        // `path` identical while `description` (produced by
        // `signature::encode` from the title) differs. Comparing only
        // target/args/working_dir would wrongly report `Skip` here and the
        // shortcut's tooltip would never refresh.
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk_with_description(
                "steam.exe",
                "-applaunch 440",
                "tf2.exe",
                "Team Fortress 2 [wso:steam:440]",
            ),
        }];
        let o = [OwnedShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk_with_description(
                "steam.exe",
                "-applaunch 440",
                "tf2.exe",
                "Team Fortress 2. [wso:steam:440]",
            ),
        }];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Retarget { .. }]));
    }
}
