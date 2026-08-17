const ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Turns a game title into a filename that NTFS accepts, preserving as much
/// of the original as possible. Trademark symbols are legal and kept.
pub fn sanitize_filename(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if ILLEGAL.contains(&c) || (c as u32) < 0x20 {
                ' '
            } else {
                c
            }
        })
        .collect();

    // Collapse runs of whitespace introduced by the replacement above.
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");

    // Windows silently strips trailing dots and spaces; do it explicitly so
    // the name we write matches the name we later read back.
    let trimmed = collapsed.trim_end_matches(['.', ' ']).trim();

    if trimmed.is_empty() {
        return "_".to_string();
    }

    if RESERVED.contains(&trimmed.to_ascii_lowercase().as_str()) {
        return format!("{trimmed}_");
    }

    trimmed.to_string()
}

use crate::platform::Platform;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Desktop,
    StartMenu,
}

impl Target {
    pub fn label(&self) -> &'static str {
        match self {
            Target::Desktop => "Desktop",
            Target::StartMenu => "Start Menu",
        }
    }
}

/// Struct variants throughout, not newtype variants: Task 11 serializes this
/// to TOML under an internal tag, which newtype variants cannot represent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", tag = "mode")]
pub enum Organization {
    /// Shortcuts sit directly in the target directory.
    Flat,
    /// One folder holds every platform's shortcuts.
    Unified { folder: String },
    /// One folder per platform, optionally nested under a shared parent.
    PerPlatform { parent: Option<String> },
}

#[derive(Debug)]
pub enum LayoutError {
    UnknownFolder(String),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::UnknownFolder(d) => {
                write!(f, "could not locate a Windows known folder: {d}")
            }
        }
    }
}

impl std::error::Error for LayoutError {}

pub fn shortcut_dir(base: &Path, org: &Organization, platform: Platform) -> PathBuf {
    match org {
        Organization::Flat => base.to_path_buf(),
        Organization::Unified { folder } => base.join(sanitize_filename(folder)),
        Organization::PerPlatform { parent } => {
            let root = match parent {
                Some(p) => base.join(sanitize_filename(p)),
                None => base.to_path_buf(),
            };
            root.join(sanitize_filename(platform.display_name()))
        }
    }
}

/// Every platform this tool currently supports.
///
/// Built through an exhaustive `match` rather than a bare list literal: the
/// arm below is required to cover every variant of `Platform`, so adding a
/// new variant to that enum makes this stop compiling until the new
/// platform is added here too. That is deliberate — `managed_dirs` (and,
/// downstream, Task 12's scan of what the tool owns) is built on top of
/// this list, and a platform silently missing from it would mean its
/// shortcuts are created but never found again for cleanup or mode
/// switches.
///
/// Public so other guarded call sites (the wizard's platform prompt, in
/// particular) can share this list instead of re-listing `Platform`
/// variants themselves with no such guard.
pub fn all_platforms() -> Vec<Platform> {
    match Platform::Steam {
        Platform::Steam | Platform::Epic => {}
    }
    vec![Platform::Steam, Platform::Epic]
}

/// Every organization variant `sync` is able to produce, purely for the
/// purpose of deciding where to *look* for shortcuts this tool already owns.
///
/// No command may rely on the currently configured layout alone. A user who
/// once organized into a unified "Games" folder and has since switched to
/// per-platform folders (or to flat, or renamed the folder) has shortcuts
/// sitting in the old layout that a scan of only today's configuration can
/// never find — so `clean` would leave them behind forever, and `sync` would
/// create a second shortcut for every game beside the ones it could not see.
/// That second failure was confirmed on a real machine: nine shortcuts
/// synced as `unified`, then a re-run as `flat` put nine more loose on the
/// Desktop while the originals stayed in the folder.
///
/// Since `Unified` and `PerPlatform` both key off an arbitrary, user-chosen
/// folder name, this can only be a best-effort list: the name currently
/// configured (if any), plus the shared default `"Games"`. A shortcut under
/// some third, no-longer-configured folder name is still not found — but
/// that is a coverage gap, not a safety one: ownership signatures
/// (`shortcut::signature`) gate every claim regardless of which directory a
/// file sits in, so widening this list can never make an unrelated, unsigned
/// shortcut eligible to be deleted or overwritten.
pub fn candidate_organizations(configured: &Organization) -> Vec<Organization> {
    let mut folder_names = vec!["Games".to_string()];
    let name = match configured {
        Organization::Unified { folder } => Some(folder.clone()),
        Organization::PerPlatform {
            parent: Some(parent),
        } => Some(parent.clone()),
        Organization::PerPlatform { parent: None } | Organization::Flat => None,
    };
    if let Some(name) = name {
        if !folder_names.contains(&name) {
            folder_names.push(name);
        }
    }

    let mut orgs = vec![
        Organization::Flat,
        Organization::PerPlatform { parent: None },
    ];
    for name in folder_names {
        orgs.push(Organization::Unified {
            folder: name.clone(),
        });
        orgs.push(Organization::PerPlatform { parent: Some(name) });
    }
    orgs
}

/// Every directory under `base` that may hold a shortcut this tool owns —
/// every layout in [`candidate_organizations`], not merely the configured
/// one — de-duplicated so a caller never reads the same directory twice.
///
/// This is what `sync`, `status` and `clean` all scan. `sync` and `status`
/// used to scan `managed_dirs(base, &cfg.organization)` instead, which is
/// what stranded and duplicated shortcuts across a mode switch; `clean` was
/// the only command that had ever been given the wider list.
pub fn scan_dirs(base: &Path, configured: &Organization) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for org in candidate_organizations(configured) {
        for d in managed_dirs(base, &org) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs
}

/// Every directory the tool may place shortcuts into under `base` for one
/// specific organization. This is where a shortcut is *written*; use
/// [`scan_dirs`] to decide where to look for shortcuts already on disk.
pub fn managed_dirs(base: &Path, org: &Organization) -> Vec<PathBuf> {
    match org {
        Organization::Flat | Organization::Unified { .. } => {
            vec![shortcut_dir(base, org, Platform::Steam)]
        }
        Organization::PerPlatform { .. } => all_platforms()
            .into_iter()
            .map(|platform| shortcut_dir(base, org, platform))
            .collect(),
    }
}

/// Produces a unique, filesystem-safe basename for each entry, in input order.
///
/// Two games can legitimately share a title across platforms, and sanitization
/// can turn two different titles into the same string, so uniqueness is
/// decided after sanitization rather than before.
pub fn assign_filenames(entries: &[(Platform, String)]) -> Vec<String> {
    let sanitized: Vec<String> = entries
        .iter()
        .map(|(_, name)| sanitize_filename(name))
        .collect();

    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for name in &sanitized {
        *counts.entry(name.as_str()).or_insert(0) += 1;
    }

    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(entries.len());

    for (i, base) in sanitized.iter().enumerate() {
        let mut candidate = if counts[base.as_str()] > 1 {
            format!("{base} ({})", entries[i].0.display_name())
        } else {
            base.clone()
        };

        let mut n = 2;
        while used.contains(&candidate) {
            candidate = format!("{base} ({}) {n}", entries[i].0.display_name());
            n += 1;
        }

        used.insert(candidate.clone());
        out.push(candidate);
    }
    out
}

#[cfg(windows)]
pub fn target_dir(target: Target) -> Result<PathBuf, LayoutError> {
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Programs, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
    };

    // FOLDERID_Programs is the per-user Start Menu. FOLDERID_CommonPrograms
    // is deliberately never used: writing there would require elevation.
    let id = match target {
        Target::Desktop => FOLDERID_Desktop,
        Target::StartMenu => FOLDERID_Programs,
    };

    unsafe {
        let pwstr = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None)
            .map_err(|e| LayoutError::UnknownFolder(e.to_string()))?;
        // Convert before freeing, but free unconditionally: `to_string`
        // copies the buffer into an owned `String`, so it is always safe to
        // free the shell-owned buffer once it returns, regardless of
        // whether the conversion succeeded. Doing the free before the `?`
        // below (rather than after, as a previous version of this function
        // did) matters because invalid UTF-16 in the returned path is a
        // real, if rare, way for `to_string` to fail — and that failure
        // must not leak the buffer `SHGetKnownFolderPath` allocated.
        let result = pwstr
            .to_string()
            .map_err(|e| LayoutError::UnknownFolder(e.to_string()));
        windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.0 as *const _));
        Ok(PathBuf::from(result?))
    }
}

#[cfg(not(windows))]
pub fn target_dir(_target: Target) -> Result<PathBuf, LayoutError> {
    Err(LayoutError::UnknownFolder("Windows only".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use std::path::PathBuf;

    #[test]
    fn strips_illegal_characters() {
        assert_eq!(sanitize_filename("X4: Foundations"), "X4 Foundations");
        assert_eq!(
            sanitize_filename("A/B\\C*D?E\"F<G>H|I"),
            "A B C D E F G H I"
        );
    }

    #[test]
    fn preserves_legal_symbols() {
        assert_eq!(sanitize_filename("Overwatch®"), "Overwatch®");
        assert_eq!(sanitize_filename("HELLDIVERS™ 2"), "HELLDIVERS™ 2");
        assert_eq!(sanitize_filename("Garry's Mod"), "Garry's Mod");
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("Trailmakers."), "Trailmakers");
        assert_eq!(sanitize_filename("Game   "), "Game");
    }

    #[test]
    fn suffixes_reserved_device_names() {
        assert_eq!(sanitize_filename("CON"), "CON_");
        assert_eq!(sanitize_filename("com1"), "com1_");
        assert_eq!(sanitize_filename("CONQUEST"), "CONQUEST");
    }

    #[test]
    fn falls_back_when_nothing_survives() {
        assert_eq!(sanitize_filename("..."), "_");
        assert_eq!(sanitize_filename(""), "_");
    }

    #[test]
    fn flat_mode_uses_the_base_directory() {
        let base = PathBuf::from(r"C:\Desktop");
        assert_eq!(
            shortcut_dir(&base, &Organization::Flat, Platform::Steam),
            base
        );
    }

    #[test]
    fn unified_mode_uses_one_folder_for_every_platform() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::Unified {
            folder: "Games".into(),
        };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("Games")
        );
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Epic),
            base.join("Games")
        );
    }

    #[test]
    fn per_platform_mode_splits_by_platform() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::PerPlatform { parent: None };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("Steam")
        );

        let nested = Organization::PerPlatform {
            parent: Some("Games".into()),
        };
        assert_eq!(
            shortcut_dir(&base, &nested, Platform::Epic),
            base.join("Games").join("Epic")
        );
    }

    #[test]
    fn folder_names_are_sanitized_too() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::Unified {
            folder: "My: Games".into(),
        };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("My Games")
        );
    }

    #[test]
    fn per_platform_parent_folder_name_is_sanitized_too() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::PerPlatform {
            parent: Some("My: Games".into()),
        };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("My Games").join("Steam")
        );
    }

    #[test]
    fn distinct_names_are_left_alone() {
        let entries = [
            (Platform::Steam, "Terraria".to_string()),
            (Platform::Epic, "Fall Guys".to_string()),
        ];
        assert_eq!(assign_filenames(&entries), vec!["Terraria", "Fall Guys"]);
    }

    #[test]
    fn cross_platform_collisions_get_a_platform_suffix() {
        let entries = [
            (Platform::Steam, "Fallout 4".to_string()),
            (Platform::Epic, "Fallout 4".to_string()),
        ];
        assert_eq!(
            assign_filenames(&entries),
            vec!["Fallout 4 (Steam)", "Fallout 4 (Epic)"]
        );
    }

    #[test]
    fn same_platform_collisions_get_a_numeric_suffix() {
        let entries = [
            (Platform::Steam, "Portal".to_string()),
            (Platform::Steam, "Portal".to_string()),
            (Platform::Steam, "Portal".to_string()),
        ];
        assert_eq!(
            assign_filenames(&entries),
            vec!["Portal (Steam)", "Portal (Steam) 2", "Portal (Steam) 3"]
        );
    }

    #[test]
    fn collisions_are_detected_after_sanitization() {
        let entries = [
            (Platform::Steam, "X4: Foundations".to_string()),
            (Platform::Epic, "X4 Foundations".to_string()),
        ];
        assert_eq!(
            assign_filenames(&entries),
            vec!["X4 Foundations (Steam)", "X4 Foundations (Epic)"]
        );
    }

    // --- candidate_organizations / scan_dirs ---

    /// The list must include the layout the user is currently configured to
    /// use — otherwise a `clean` run right after a normal `sync` could miss
    /// the very shortcuts that sync just created, which would be a far more
    /// common failure than the stale-layout scenario this function primarily
    /// exists for.
    #[test]
    fn candidate_organizations_always_includes_the_currently_configured_layout() {
        let configured = Organization::PerPlatform {
            parent: Some("Juegos".into()),
        };
        let orgs = candidate_organizations(&configured);
        assert!(
            orgs.contains(&configured),
            "expected the configured PerPlatform(\"Juegos\") layout among {orgs:?}"
        );
    }

    /// The whole point: a user who *used to* organize into a unified "Games"
    /// folder, and is now configured for something else entirely, must still
    /// have "Games" scanned so old shortcuts are neither stranded nor
    /// duplicated. This is the scenario a scan of only the current config
    /// cannot detect.
    #[test]
    fn candidate_organizations_includes_the_default_games_folder_even_when_not_configured() {
        let orgs = candidate_organizations(&Organization::PerPlatform {
            parent: Some("Juegos".into()),
        });
        assert!(
            orgs.contains(&Organization::Unified {
                folder: "Games".into()
            }),
            "expected the default Unified(\"Games\") layout among {orgs:?}"
        );
        assert!(
            orgs.contains(&Organization::PerPlatform {
                parent: Some("Games".into())
            }),
            "expected the default PerPlatform(\"Games\") layout among {orgs:?}"
        );
    }

    /// Flat mode and parent-less per-platform mode do not depend on any
    /// folder name at all, so they must always be scanned regardless of what
    /// is currently configured.
    #[test]
    fn candidate_organizations_always_includes_flat_and_parentless_per_platform() {
        let orgs = candidate_organizations(&Organization::Flat);
        assert!(orgs.contains(&Organization::Flat));
        assert!(orgs.contains(&Organization::PerPlatform { parent: None }));
    }

    /// When the configured folder name already *is* "Games" (the shared
    /// default), the list must not contain two identical
    /// `Unified { folder: "Games" }` entries — proving the de-duplication
    /// keys on the folder name, not merely on "is this the default".
    #[test]
    fn candidate_organizations_does_not_duplicate_when_configured_name_is_the_default() {
        let orgs = candidate_organizations(&Organization::Unified {
            folder: "Games".into(),
        });
        let games_unified_count = orgs
            .iter()
            .filter(|o| matches!(o, Organization::Unified { folder } if folder == "Games"))
            .count();
        assert_eq!(
            games_unified_count, 1,
            "expected exactly one Unified(\"Games\") entry, found {games_unified_count} in {orgs:?}"
        );
    }

    /// The directory a `flat` run writes into is `base` itself. Configured
    /// for `unified`, `managed_dirs` names only `base\Games` — so a shortcut
    /// left loose on the Desktop by an earlier flat run is invisible, and
    /// `sync` creates a duplicate beside it instead of moving it. `scan_dirs`
    /// must name `base` too.
    #[test]
    fn scan_dirs_includes_the_flat_base_even_when_configured_for_a_folder() {
        let base = PathBuf::from(r"C:\Desktop");
        let configured = Organization::Unified {
            folder: "Games".into(),
        };
        assert_eq!(managed_dirs(&base, &configured), vec![base.join("Games")]);
        assert!(
            scan_dirs(&base, &configured).contains(&base),
            "a shortcut from a previous flat run sits directly in the base and must be scanned"
        );
    }

    /// The reverse direction, and the per-platform layouts: configured for
    /// `flat`, the scan must still descend into the folders a `unified` or
    /// `per-platform` run would have written into — including the
    /// user-chosen folder name, not just the default.
    #[test]
    fn scan_dirs_includes_the_folder_layouts_even_when_configured_flat() {
        let base = PathBuf::from(r"C:\Desktop");
        let dirs = scan_dirs(&base, &Organization::Flat);
        for expected in [
            base.join("Games"),
            base.join("Steam"),
            base.join("Epic"),
            base.join("Games").join("Steam"),
            base.join("Games").join("Epic"),
        ] {
            assert!(
                dirs.contains(&expected),
                "expected {expected:?} among {dirs:?}"
            );
        }
    }

    /// A user-chosen folder name must be scanned as well as the default —
    /// this is the only entry that depends on the *configured* layout rather
    /// than the fixed candidate list.
    #[test]
    fn scan_dirs_includes_a_user_chosen_folder_name() {
        let base = PathBuf::from(r"C:\Desktop");
        let dirs = scan_dirs(
            &base,
            &Organization::Unified {
                folder: "Juegos".into(),
            },
        );
        assert!(dirs.contains(&base.join("Juegos")), "{dirs:?}");
        assert!(dirs.contains(&base.join("Games")), "{dirs:?}");
    }

    /// Several candidate layouts name the same directory — `Flat` and
    /// `Unified` both reduce to `base` when the folder name is empty of
    /// effect, and the configured name may equal the default. Reading the
    /// same directory twice would let one shortcut be claimed twice and
    /// reconciled against itself, so the list must be free of duplicates.
    #[test]
    fn scan_dirs_never_names_the_same_directory_twice() {
        let base = PathBuf::from(r"C:\Desktop");
        for configured in [
            Organization::Flat,
            Organization::Unified {
                folder: "Games".into(),
            },
            Organization::PerPlatform { parent: None },
            Organization::PerPlatform {
                parent: Some("Games".into()),
            },
        ] {
            let dirs = scan_dirs(&base, &configured);
            let mut unique = dirs.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), dirs.len(), "duplicates in {dirs:?}");
        }
    }

    #[test]
    fn managed_dirs_lists_every_directory_we_may_own() {
        let base = PathBuf::from(r"C:\Desktop");
        assert_eq!(managed_dirs(&base, &Organization::Flat), vec![base.clone()]);
        assert_eq!(
            managed_dirs(
                &base,
                &Organization::Unified {
                    folder: "Games".into()
                }
            ),
            vec![base.join("Games")]
        );

        // Derived from `all_platforms()` — the same source `managed_dirs`
        // itself consults — rather than a hardcoded two-item list, so this
        // test keeps proving "every platform" instead of just "these two
        // platforms" if a third one is ever added.
        let org = Organization::PerPlatform {
            parent: Some("Games".into()),
        };
        let expected: Vec<PathBuf> = all_platforms()
            .into_iter()
            .map(|platform| base.join("Games").join(platform.display_name()))
            .collect();
        assert_eq!(managed_dirs(&base, &org), expected);
    }

    #[cfg(windows)]
    #[test]
    fn resolves_per_user_known_folders() {
        let desktop = target_dir(Target::Desktop).unwrap();
        let start = target_dir(Target::StartMenu).unwrap();
        assert!(
            desktop.is_dir(),
            "desktop should exist: {}",
            desktop.display()
        );
        assert!(
            start.is_dir(),
            "start menu should exist: {}",
            start.display()
        );
        assert!(
            start.to_string_lossy().contains("AppData"),
            "must be the per-user Start Menu, got {}",
            start.display()
        );
    }
}
