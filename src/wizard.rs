//! Interactive prompts. The translation from answers to configuration is
//! kept pure and separate from the prompting so it can be tested: `run`
//! collects `Answers` through `inquire` (which needs a real terminal and so
//! cannot be driven from a test) and does nothing else until it hands them,
//! all at once, to `apply_answers`. That function is the only place that
//! decides how answers become configuration, and it never touches I/O.

use crate::config::Config;
use crate::layout::{Organization, Target};
use crate::platform::Platform;

/// Returned when the user backs out of the wizard (Escape or Ctrl-C on any
/// prompt) or deliberately chooses no platforms. `run` never calls
/// `apply_answers` in either case, so `cfg` is left exactly as it was.
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

/// Runs the interactive wizard: platforms, then targets, then organization,
/// then (when it means something) a folder name. Nothing in `cfg` changes
/// until every prompt has succeeded — the answers are accumulated in local
/// variables and only handed to `apply_answers` once, at the very end, so a
/// cancellation on any prompt (`?` below) returns before `cfg` is touched at
/// all, leaving it exactly as the caller passed it in.
pub fn run(cfg: &mut Config) -> Result<(), Cancelled> {
    use inquire::{Confirm, MultiSelect, Select, Text};

    println!("WinShortcutsOrganizer setup\n");

    let platform_options = vec![Platform::Steam, Platform::Epic];
    let labels: Vec<&str> = platform_options.iter().map(|p| p.display_name()).collect();
    let defaults: Vec<usize> = (0..labels.len()).collect();
    let chosen = MultiSelect::new("Which platforms should be scanned?", labels)
        .with_default(&defaults)
        .prompt()
        .map_err(|_| Cancelled)?;

    if chosen.is_empty() {
        println!(
            "\nNo platforms selected — there would be nothing to sync, so setup is cancelled.\n\
             Run the wizard again to choose at least one platform."
        );
        return Err(Cancelled);
    }
    let platforms: Vec<Platform> = platform_options
        .iter()
        .copied()
        .filter(|p| chosen.contains(&p.display_name()))
        .collect();

    let target_choice = Select::new(
        "Where should shortcuts be placed?",
        vec!["Start Menu", "Desktop", "Both"],
    )
    .prompt()
    .map_err(|_| Cancelled)?;
    let targets = match target_choice {
        "Desktop" => vec![Target::Desktop],
        "Both" => vec![Target::Desktop, Target::StartMenu],
        _ => vec![Target::StartMenu],
    };

    let org_choice = Select::new(
        "How should shortcuts be grouped?",
        vec![
            "One folder for all games",
            "One folder per platform",
            "No folder — place them directly in the target",
        ],
    )
    .prompt()
    .map_err(|_| Cancelled)?;
    let organization = match org_choice {
        "One folder per platform" => OrgChoice::PerPlatform,
        "No folder — place them directly in the target" => OrgChoice::Flat,
        _ => OrgChoice::Unified,
    };

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

    #[test]
    fn preserves_exclusions_and_overrides() {
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

    /// The wizard never asks about icon overrides, so — like exclusions
    /// above — a hand-set override must still be there afterwards.
    /// `apply_answers` simply never assigns `cfg.icon_overrides`; this test
    /// is what would catch a future edit that started doing so.
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
}
