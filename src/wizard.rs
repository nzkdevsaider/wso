//! Interactive prompts. The translation from answers to configuration is
//! kept pure and separate from the prompting so it can be tested: `run`
//! collects `Answers` through `inquire` (which needs a real terminal and so
//! cannot be driven from a test) and does nothing else until it hands them,
//! all at once, to `apply_answers`. That function is the only place that
//! decides how answers become configuration, and it never touches I/O.
//!
//! Every prompt in `run` is backed by a table (`ORGANIZATION_OPTIONS`,
//! `TARGET_OPTIONS`, `platform_table`) that is the single source of truth
//! for both the option labels shown to the user and the meaning assigned to
//! whichever one they pick. Earlier revisions of this file matched on label
//! strings in two separate places — the prompt's option list and a `match`
//! interpreting the answer — which meant editing one without the other (a
//! typo, a reworded label, an em dash quietly becoming a hyphen) silently
//! changed what the user saw without changing what the code understood, with
//! nothing failing to compile and no test failing. Reading both the options
//! and their meaning from one table closes that gap, and a lookup miss is
//! treated as a cancellation rather than a guessed default.

use crate::config::Config;
use crate::layout::{Organization, Target};
use crate::platform::Platform;

/// Returned when the user backs out of the wizard (Escape or Ctrl-C on any
/// prompt), deliberately chooses no platforms, or — defensively — when a
/// prompt's answer fails to resolve against its table (see the module doc).
/// `run` never calls `apply_answers` in any of these cases, so `cfg` is left
/// exactly as it was.
#[derive(Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Which organization mode the user picked, before it has been turned into
/// an `Organization` value. Kept distinct from `Organization` itself so the
/// wizard can carry a `folder` answer alongside it and let `apply_answers`
/// decide, per variant, whether that answer means anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgChoice {
    Flat,
    Unified,
    PerPlatform,
}

/// Everything the wizard asked the user, gathered before any of it is
/// applied to a `Config`. `folder` is collected once regardless of
/// `organization`'s value; `apply_answers` is what decides whether it is
/// meaningful, defaulted, or discarded — never the prompting code.
#[derive(Debug, Clone)]
pub struct Answers {
    pub platforms: Vec<Platform>,
    pub targets: Vec<Target>,
    pub organization: OrgChoice,
    pub folder: Option<String>,
}

/// Turns `a` into the fields of `cfg` the wizard is responsible for —
/// `platforms`, `targets` and `organization` — and nothing else. `exclusions`
/// and `icon_overrides` are never touched here, because the wizard never
/// asks about them: a user who hand-edited either must not have them
/// silently reset just for running the wizard afterwards.
///
/// The folder name is applied per organization mode rather than trusted
/// verbatim: `Flat` has no folder at all, so `a.folder` (whatever it holds)
/// is dropped; `Unified` always needs one, so a missing answer falls back to
/// `"Games"`; `PerPlatform`'s parent is genuinely optional, so `a.folder` is
/// carried through as-is, `None` included. This is also what stops a folder
/// name collected for one mode leaking into a different one: each arm reads
/// only `a.folder`, and only the arm matching `a.organization` runs at all.
pub fn apply_answers(cfg: &mut Config, a: Answers) {
    cfg.platforms = a.platforms;
    cfg.targets = a.targets;
    cfg.organization = match a.organization {
        OrgChoice::Flat => Organization::Flat,
        OrgChoice::Unified => {
            Organization::Unified { folder: a.folder.unwrap_or_else(|| "Games".into()) }
        }
        OrgChoice::PerPlatform => Organization::PerPlatform { parent: a.folder },
    };
}

/// Label ↔ meaning pairs for the "how should shortcuts be grouped" prompt —
/// the single source of truth used both to render the prompt's option list
/// (`organization_labels`) and to interpret the answer (`resolve_organization`).
const ORGANIZATION_OPTIONS: [(&str, OrgChoice); 3] = [
    ("One folder for all games", OrgChoice::Unified),
    ("One folder per platform", OrgChoice::PerPlatform),
    ("No folder — place them directly in the target", OrgChoice::Flat),
];

fn organization_labels() -> Vec<&'static str> {
    ORGANIZATION_OPTIONS.iter().map(|(label, _)| *label).collect()
}

/// Looks `choice` up in `ORGANIZATION_OPTIONS` rather than trusting it blindly.
/// A `Select` prompt built from `organization_labels()` can only ever return
/// one of that table's own labels, so this lookup cannot fail in practice —
/// but if the table and the prompt ever drift apart, a miss becomes a
/// cancellation, not a silently guessed `OrgChoice::Unified`.
fn resolve_organization(choice: &str) -> Result<OrgChoice, Cancelled> {
    ORGANIZATION_OPTIONS
        .iter()
        .find(|(label, _)| *label == choice)
        .map(|(_, org)| *org)
        .ok_or(Cancelled)
}

/// Label ↔ targets pairs for the "where should shortcuts go" prompt — the
/// single source of truth used both to render the prompt's option list
/// (`target_labels`) and to interpret the answer (`resolve_targets`).
const TARGET_OPTIONS: [(&str, &[Target]); 3] = [
    ("Start Menu", &[Target::StartMenu]),
    ("Desktop", &[Target::Desktop]),
    ("Both", &[Target::Desktop, Target::StartMenu]),
];

fn target_labels() -> Vec<&'static str> {
    TARGET_OPTIONS.iter().map(|(label, _)| *label).collect()
}

/// Looks `choice` up in `TARGET_OPTIONS` rather than trusting it blindly, for
/// the same reason as `resolve_organization`: a miss becomes a cancellation,
/// never a guessed default target list.
fn resolve_targets(choice: &str) -> Result<Vec<Target>, Cancelled> {
    TARGET_OPTIONS
        .iter()
        .find(|(label, _)| *label == choice)
        .map(|(_, targets)| targets.to_vec())
        .ok_or(Cancelled)
}

/// Label ↔ `Platform` pairs for the platform prompt, built from
/// `layout::all_platforms()` rather than a second hardcoded list of
/// variants. `all_platforms` is an exhaustive match over `Platform`, so a
/// platform added there without updating this table is a compile error —
/// the same guard the crate already relies on for `layout::managed_dirs`,
/// shared here instead of re-implemented (and left unguarded) a second time.
fn platform_table() -> Vec<(&'static str, Platform)> {
    crate::layout::all_platforms().into_iter().map(|p| (p.display_name(), p)).collect()
}

fn platform_labels() -> Vec<&'static str> {
    platform_table().iter().map(|(label, _)| *label).collect()
}

/// Pure: turns the labels the user checked in the platform `MultiSelect`
/// back into `Platform`s. The result follows `platform_table`'s own order,
/// not `chosen`'s, so checking boxes in any order yields the same
/// `Vec<Platform>` as long as the same boxes are checked.
fn resolve_platforms(chosen: &[&str]) -> Vec<Platform> {
    platform_table()
        .into_iter()
        .filter(|(label, _)| chosen.contains(label))
        .map(|(_, platform)| platform)
        .collect()
}

/// True when the platform `MultiSelect` came back with nothing checked. This
/// is a real, deliberate input — not an error — so `run` gives it its own
/// branch instead of letting an empty `Vec<Platform>` fall through to
/// `apply_answers` unremarked; see `run` for what happens when it's true.
fn no_platforms_chosen(chosen: &[&str]) -> bool {
    chosen.is_empty()
}

/// Runs the interactive wizard: platforms, then targets, then organization,
/// then (when it means something) a folder name. Every decision — which
/// platforms were checked, which targets a label means, which `OrgChoice` a
/// label means — is delegated to a pure function above; this function only
/// prompts and sequences. Nothing in `cfg` changes until every prompt (and
/// every lookup) has succeeded: the answers are accumulated in local
/// variables and only handed to `apply_answers` once, at the very end, so a
/// cancellation on any prompt or lookup (the `?` operators below) returns
/// before `cfg` is touched at all, leaving it exactly as the caller passed
/// it in.
pub fn run(cfg: &mut Config) -> Result<(), Cancelled> {
    use inquire::{Confirm, MultiSelect, Select, Text};

    println!("WinShortcutsOrganizer setup\n");

    let labels = platform_labels();
    let defaults: Vec<usize> = (0..labels.len()).collect();
    let chosen = MultiSelect::new("Which platforms should be scanned?", labels)
        .with_default(&defaults)
        .prompt()
        .map_err(|_| Cancelled)?;

    if no_platforms_chosen(&chosen) {
        println!(
            "\nNo platforms selected — there would be nothing to sync, so setup is cancelled.\n\
             Run the wizard again to choose at least one platform."
        );
        return Err(Cancelled);
    }
    let platforms = resolve_platforms(&chosen);

    let target_choice = Select::new("Where should shortcuts be placed?", target_labels())
        .prompt()
        .map_err(|_| Cancelled)?;
    let targets = resolve_targets(target_choice)?;

    let org_choice = Select::new("How should shortcuts be grouped?", organization_labels())
        .prompt()
        .map_err(|_| Cancelled)?;
    let organization = resolve_organization(org_choice)?;

    let folder = match organization {
        OrgChoice::Flat => None,
        OrgChoice::Unified => Some(
            Text::new("Folder name:")
                .with_default("Games")
                .prompt()
                .map_err(|_| Cancelled)?,
        ),
        OrgChoice::PerPlatform => {
            let nest = Confirm::new("Nest the per-platform folders inside a parent folder?")
                .with_default(true)
                .prompt()
                .map_err(|_| Cancelled)?;
            if nest {
                Some(
                    Text::new("Parent folder name:")
                        .with_default("Games")
                        .prompt()
                        .map_err(|_| Cancelled)?,
                )
            } else {
                None
            }
        }
    };

    let start_menu_chosen = targets.contains(&Target::StartMenu);

    apply_answers(cfg, Answers { platforms, targets, organization, folder });

    if start_menu_chosen {
        println!(
            "\nNote: Windows does not allow programs to pin items to the Start menu panel.\n\
             Your games will appear under All apps and in search; pinning them to the Start\n\
             panel is a manual right-click from there."
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Organization, Target};
    use crate::platform::Platform;

    #[test]
    fn builds_config_from_answers() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam],
                targets: vec![Target::Desktop, Target::StartMenu],
                organization: OrgChoice::PerPlatform,
                folder: Some("Juegos".into()),
            },
        );
        assert_eq!(cfg.platforms, vec![Platform::Steam]);
        assert_eq!(cfg.targets.len(), 2);
        assert_eq!(
            cfg.organization,
            Organization::PerPlatform { parent: Some("Juegos".into()) }
        );
    }

    #[test]
    fn flat_mode_ignores_the_folder_name() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam, Platform::Epic],
                targets: vec![Target::StartMenu],
                organization: OrgChoice::Flat,
                folder: Some("Ignored".into()),
            },
        );
        assert_eq!(cfg.organization, Organization::Flat);
    }

    #[test]
    fn unified_mode_defaults_the_folder_name() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Epic],
                targets: vec![Target::StartMenu],
                organization: OrgChoice::Unified,
                folder: None,
            },
        );
        assert_eq!(
            cfg.organization,
            Organization::Unified { folder: "Games".into() }
        );
    }

    /// The `PerPlatform` parent is genuinely optional (see `apply_answers`):
    /// unlike `Unified`, a missing folder answer here must stay `None`
    /// rather than being defaulted to anything. Only the `Some` case was
    /// covered before this test — the "don't nest" outcome, which is the
    /// simpler of the two, was not.
    #[test]
    fn per_platform_mode_allows_no_parent_folder() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam],
                targets: vec![Target::StartMenu],
                organization: OrgChoice::PerPlatform,
                folder: None,
            },
        );
        assert_eq!(cfg.organization, Organization::PerPlatform { parent: None });
    }

    /// The wizard never asks about exclusions, so a hand-set exclusion list
    /// must still be there afterwards. `apply_answers` simply never assigns
    /// `cfg.exclusions`; this test is what would catch a future edit that
    /// started doing so.
    #[test]
    fn preserves_exclusions() {
        let mut cfg = Config::default();
        cfg.exclusions = vec!["steam:1".into()];
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam],
                targets: vec![Target::Desktop],
                organization: OrgChoice::Flat,
                folder: None,
            },
        );
        assert_eq!(cfg.exclusions, vec!["steam:1".to_string()]);
    }

    /// The wizard never asks about icon overrides either, so — like
    /// exclusions above — a hand-set override must still be there
    /// afterwards. `apply_answers` simply never assigns `cfg.icon_overrides`;
    /// this test is what would catch a future edit that started doing so.
    #[test]
    fn preserves_icon_overrides() {
        use std::path::PathBuf;

        let mut cfg = Config::default();
        cfg.icon_overrides.insert("steam:440".into(), PathBuf::from(r"C:\icons\tf2.ico"));
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Epic],
                targets: vec![Target::StartMenu],
                organization: OrgChoice::Unified,
                folder: None,
            },
        );
        assert_eq!(
            cfg.icon_overrides.get("steam:440"),
            Some(&PathBuf::from(r"C:\icons\tf2.ico"))
        );
    }

    /// Each label in `ORGANIZATION_OPTIONS` must map back to the `OrgChoice`
    /// paired with it in that same table — proving `resolve_organization`
    /// actually reads the table rather than containing its own separate,
    /// possibly-drifted `match`.
    #[test]
    fn resolve_organization_maps_each_table_label_to_its_own_choice() {
        for (label, expected) in ORGANIZATION_OPTIONS {
            assert_eq!(resolve_organization(label), Ok(expected));
        }
    }

    /// A label that isn't in the table (the only way this can happen in
    /// practice is the table and the `Select` prompt drifting apart — see
    /// the module doc) must cancel rather than silently resolve to some
    /// default `OrgChoice`.
    #[test]
    fn resolve_organization_rejects_a_label_absent_from_the_table() {
        assert_eq!(resolve_organization("Some option nobody offered"), Err(Cancelled));
    }

    /// Same guarantee as `resolve_organization_maps_each_table_label_to_its_own_choice`,
    /// for the target prompt's table.
    #[test]
    fn resolve_targets_maps_each_table_label_to_its_own_targets() {
        for (label, expected) in TARGET_OPTIONS {
            assert_eq!(resolve_targets(label), Ok(expected.to_vec()));
        }
    }

    #[test]
    fn resolve_targets_rejects_a_label_absent_from_the_table() {
        assert_eq!(resolve_targets("Neither, thanks"), Err(Cancelled));
    }

    /// `resolve_platforms` must return exactly the platforms whose labels
    /// were checked — not more, not fewer — and in `platform_table`'s own
    /// order rather than the order they happen to appear in `chosen`, which
    /// is proved here by listing them checked in reverse.
    #[test]
    fn resolve_platforms_returns_checked_platforms_in_table_order_regardless_of_input_order() {
        assert_eq!(resolve_platforms(&["Epic", "Steam"]), vec![Platform::Steam, Platform::Epic]);
        assert_eq!(resolve_platforms(&["Epic"]), vec![Platform::Epic]);
    }

    #[test]
    fn resolve_platforms_returns_empty_for_an_empty_selection() {
        assert_eq!(resolve_platforms(&[]), Vec::<Platform>::new());
    }

    #[test]
    fn no_platforms_chosen_is_true_only_for_an_empty_selection() {
        assert!(no_platforms_chosen(&[]));
        assert!(!no_platforms_chosen(&["Steam"]));
    }
}
