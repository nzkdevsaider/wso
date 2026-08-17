//! Pure reconciliation between the shortcuts that should exist and the
//! shortcuts this tool already owns. No I/O happens here, which is what makes
//! every command's behavior testable without Windows, Steam or Epic.

use crate::shortcut::signature::Signature;
use crate::shortcut::LnkData;
use std::path::PathBuf;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create(DesiredShortcut),
    Retarget { path: PathBuf, lnk: LnkData },
    FixIcon { path: PathBuf, lnk: LnkData },
    Move { from: PathBuf, to: PathBuf, lnk: LnkData },
    DeleteOrphan(PathBuf),
    Skip { path: PathBuf, reason: SkipReason },
}

/// Reconciles what should exist against what this tool already owns.
///
/// Matching is by signature, never by path or filename, so a shortcut the
/// user renamed or moved is still recognized as the same game.
pub fn plan(desired: &[DesiredShortcut], owned: &[OwnedShortcut]) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut matched: Vec<bool> = vec![false; owned.len()];

    for d in desired {
        let existing = owned.iter().position(|o| o.sig == d.sig);

        let Some(idx) = existing else {
            actions.push(Action::Create(d.clone()));
            continue;
        };
        matched[idx] = true;
        let o = &owned[idx];

        // A relocation rewrites the file at the new path, so it subsumes any
        // target or icon change and must be checked first.
        if o.path != d.path {
            actions.push(Action::Move {
                from: o.path.clone(),
                to: d.path.clone(),
                lnk: d.lnk.clone(),
            });
        } else if o.lnk.target != d.lnk.target
            || o.lnk.args != d.lnk.args
            || o.lnk.working_dir != d.lnk.working_dir
        {
            actions.push(Action::Retarget { path: d.path.clone(), lnk: d.lnk.clone() });
        } else if o.lnk.icon != d.lnk.icon {
            actions.push(Action::FixIcon { path: d.path.clone(), lnk: d.lnk.clone() });
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
        Signature { platform: Platform::Steam, id: id.into() }
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
            [Action::Skip { reason: SkipReason::UpToDate, .. }]
        ));
    }

    #[test]
    fn is_idempotent() {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let first = plan(&d, &o);
        let second = plan(&d, &o);
        assert_eq!(first.len(), second.len());
        assert!(first.iter().all(|a| matches!(a, Action::Skip { .. })));
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
            sig: Signature { platform: Platform::Epic, id: "440".into() },
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
            desired("440", r"C:\D\TF2.lnk", "tf2.exe"),   // up to date
            desired("570", r"C:\D\Dota.lnk", "new.exe"),  // icon changed
            desired("620", r"C:\D\P2.lnk", "p2.exe"),     // new
        ];
        let o = [
            owned("440", r"C:\D\TF2.lnk", "tf2.exe"),
            owned("570", r"C:\D\Dota.lnk", "old.exe"),
            owned("999", r"C:\D\Gone.lnk", "gone.exe"),   // uninstalled
        ];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 4);
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::Skip { .. })).count(),
            1
        );
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::FixIcon { .. })).count(),
            1
        );
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::Create(_))).count(),
            1
        );
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::DeleteOrphan(_))).count(),
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
            Action::Skip { reason: SkipReason::UpToDate, .. }
        ));
        assert!(matches!(
            &actions[1],
            Action::DeleteOrphan(p) if p == &PathBuf::from(r"C:\D\TF2 - Copy.lnk")
        ));
    }
}
