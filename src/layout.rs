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

/// Every directory the tool may place shortcuts into under `base` for one
/// specific organization. This is where a shortcut is *written*.
///
/// Deliberately not used to decide where to *look*: a scan driven by the
/// configured organization can only ever find the layout configured right
/// now, and the folder name a previous run used is not recoverable from the
/// configuration once it has been replaced. `ops::scan_dirs` reads the
/// filesystem for that instead.
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

        let org = Organization::PerPlatform {
            parent: Some("Games".into()),
        };

        // Two assertions, because either one alone is weak in a different
        // way. The derived expectation keeps proving "every platform" rather
        // than "these two platforms" if a third is ever added — but it is
        // built from `all_platforms()`, the very source `managed_dirs`
        // consults, so a fault in that list is reproduced identically on both
        // sides and cancels out. The hardcoded list below cannot cancel out:
        // it is an independent statement of what these directories must
        // actually be, so a platform silently dropped from `all_platforms()`
        // fails here even though the derived assertion would still pass.
        let expected: Vec<PathBuf> = all_platforms()
            .into_iter()
            .map(|platform| base.join("Games").join(platform.display_name()))
            .collect();
        assert_eq!(managed_dirs(&base, &org), expected);

        assert_eq!(
            managed_dirs(&base, &org),
            vec![
                base.join("Games").join("Steam"),
                base.join("Games").join("Epic"),
            ],
            "the directories this tool actually writes into today, stated \
             independently of the list it derives them from"
        );
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
