use crate::layout::{Organization, Target};
use crate::platform::Platform;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Serialize(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(d) => write!(f, "could not write config: {d}"),
            ConfigError::Serialize(d) => write!(f, "could not serialize config: {d}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Persisted user choices. This file holds preferences only — never ownership.
/// Which shortcuts belong to the tool is recorded in the shortcuts themselves,
/// in their description field, so that a shortcut moved or deleted by hand
/// never leaves this file out of sync with reality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub targets: Vec<Target>,
    pub organization: Organization,
    pub platforms: Vec<Platform>,
    /// Either a qualified id (`steam:228980`) or a case-insensitive game name.
    pub exclusions: Vec<String>,
    /// Qualified id to an icon file, for the cases the heuristic gets wrong.
    pub icon_overrides: HashMap<String, PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            targets: vec![Target::StartMenu],
            organization: Organization::Unified { folder: "Games".into() },
            platforms: vec![Platform::Steam, Platform::Epic],
            exclusions: Vec::new(),
            icon_overrides: HashMap::new(),
        }
    }
}

impl Config {
    /// An exclusion is either a qualified id (`steam:228980`, platform-specific)
    /// or a bare game name (case-insensitive, matches on any platform). Entries
    /// are trimmed before comparison so stray whitespace from a hand-edited
    /// file doesn't silently defeat them. The qualified form is never split on
    /// its colon, so a name that itself contains one (e.g. "X4: Foundations")
    /// is only ever compared whole against `name`, never misread as a
    /// platform prefix.
    pub fn is_excluded(&self, platform: Platform, id: &str, name: &str) -> bool {
        let qualified = format!("{}:{}", platform.as_str(), id);
        self.exclusions.iter().any(|e| {
            let e = e.trim();
            e.eq_ignore_ascii_case(&qualified) || e.eq_ignore_ascii_case(name)
        })
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("WinShortcutsOrganizer").join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("WinShortcutsOrganizer").join("icons")
}

/// A missing or corrupt config is not an error: the tool falls back to
/// defaults and will write a good file back out on the next save.
pub fn load(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(cfg).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    std::fs::write(path, text).map_err(|e| ConfigError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a scratch directory path under the OS temp dir that nothing
    /// else can claim: `label` combined with this process's id and an
    /// atomically incremented counter, so the name is unique both across
    /// concurrent `cargo test` processes (e.g. two collaborators or CI jobs
    /// sharing a runner) and across parallel test threads within one
    /// process; a fixed name would let them collide.
    fn scratch_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("wso_config_{label}_{pid}_{n}"))
    }

    #[test]
    fn defaults_are_sensible() {
        let c = Config::default();
        assert_eq!(c.targets, vec![Target::StartMenu]);
        assert_eq!(c.organization, Organization::Unified { folder: "Games".into() });
        assert_eq!(c.platforms, vec![Platform::Steam, Platform::Epic]);
        assert!(c.exclusions.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let dir = scratch_dir("roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut c = Config::default();
        c.targets = vec![Target::Desktop, Target::StartMenu];
        c.organization = Organization::PerPlatform { parent: Some("Games".into()) };
        c.exclusions = vec!["steam:228980".into(), "Wallpaper Engine".into()];
        c.icon_overrides
            .insert("steam:440".into(), PathBuf::from(r"C:\icons\tf2.ico"));

        save(&path, &c).unwrap();
        assert_eq!(load(&path), c);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The icon override path contains a backslash, which TOML's basic
    /// string syntax treats as an escape character. If `save` ever wrote
    /// this as a bare basic string instead of a literal string (or with
    /// escaping), `C:\icons\tf2.ico` would come back mangled (e.g.
    /// `C:iconsf2.ico`) even though the rest of the round trip passed. This
    /// test isolates that one field so a regression there cannot hide behind
    /// the broader round-trip assertion above.
    #[test]
    fn windows_path_with_backslashes_survives_save_and_load_intact() {
        let dir = scratch_dir("backslash_path");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut c = Config::default();
        c.icon_overrides
            .insert("steam:440".into(), PathBuf::from(r"C:\icons\tf2.ico"));

        save(&path, &c).unwrap();
        let loaded = load(&path);
        assert_eq!(
            loaded.icon_overrides.get("steam:440"),
            Some(&PathBuf::from(r"C:\icons\tf2.ico"))
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Saving twice in a row (as happens whenever the wizard runs, then the
    /// user hand-edits the file, then the tool saves again) must not drift:
    /// the second save's bytes must match the first, proving `load` doesn't
    /// silently drop or reshape a field before it gets written back out.
    #[test]
    fn saving_a_loaded_config_reproduces_the_same_bytes() {
        let dir = scratch_dir("save_load_save");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut c = Config::default();
        c.targets = vec![Target::Desktop];
        c.organization = Organization::Flat;
        c.exclusions = vec!["steam:1".into()];
        c.icon_overrides.insert("epic:foo".into(), PathBuf::from(r"D:\a\b.ico"));

        save(&path, &c).unwrap();
        let first_bytes = std::fs::read_to_string(&path).unwrap();

        let reloaded = load(&path);
        save(&path, &reloaded).unwrap();
        let second_bytes = std::fs::read_to_string(&path).unwrap();

        assert_eq!(first_bytes, second_bytes);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        assert_eq!(load(Path::new(r"C:\nope\wso_config_definitely_missing\config.toml")), Config::default());
    }

    #[test]
    fn a_corrupt_file_yields_defaults() {
        let dir = scratch_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        assert_eq!(load(&path), Config::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exclusions_match_by_qualified_id_or_by_name() {
        let mut c = Config::default();
        c.exclusions = vec!["steam:228980".into(), "Wallpaper Engine".into()];

        assert!(c.is_excluded(Platform::Steam, "228980", "Redistributables"));
        assert!(c.is_excluded(Platform::Steam, "431960", "Wallpaper Engine"));
        assert!(c.is_excluded(Platform::Steam, "431960", "wallpaper engine"));
        assert!(!c.is_excluded(Platform::Epic, "228980", "Something Else"));
        assert!(!c.is_excluded(Platform::Steam, "105600", "Terraria"));
    }

    /// A qualified-id exclusion is platform-specific: `steam:228980` names
    /// Steam's app 228980, and must not accidentally match an Epic game that
    /// happens to share the numeric id "228980". Only the name form is
    /// meant to be platform-agnostic.
    #[test]
    fn qualified_id_exclusion_does_not_leak_across_platforms() {
        let mut c = Config::default();
        c.exclusions = vec!["steam:228980".into()];

        assert!(c.is_excluded(Platform::Steam, "228980", "Redistributables"));
        assert!(!c.is_excluded(Platform::Epic, "228980", "Redistributables"));
    }

    /// A game whose name itself contains a colon (e.g. "X4: Foundations")
    /// must still be excludable by name, and must not be misread as a
    /// qualified-id exclusion for some platform named "x4".
    #[test]
    fn a_name_containing_a_colon_is_still_matched_as_a_plain_name() {
        let mut c = Config::default();
        c.exclusions = vec!["X4: Foundations".into()];

        assert!(c.is_excluded(Platform::Steam, "392160", "X4: Foundations"));
        assert!(c.is_excluded(Platform::Epic, "some-id", "x4: foundations"));
    }

    /// A hand-edited config file is likely to pick up stray leading or
    /// trailing whitespace around an entry. That must not silently defeat
    /// the exclusion.
    #[test]
    fn whitespace_padded_exclusions_still_match() {
        let mut c = Config::default();
        c.exclusions = vec!["  steam:228980  ".into(), " Wallpaper Engine ".into()];

        assert!(c.is_excluded(Platform::Steam, "228980", "Redistributables"));
        assert!(c.is_excluded(Platform::Steam, "1", "Wallpaper Engine"));
    }
}
