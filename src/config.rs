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

/// What happened when `load` tried to read a config file. `Config::default()`
/// is returned alongside both `Missing` and `Corrupt`, but the two are not
/// the same situation: a missing file is expected on first run and there is
/// nothing to lose, while a corrupt file means the user's own preferences
/// are being silently discarded and the *next* `save` would otherwise
/// overwrite them with defaults for good. Carrying this status forward lets
/// a caller warn on `Corrupt` without having to re-implement the read+parse
/// itself.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigStatus {
    /// No file at that path. Defaults are correct and expected on first run.
    Missing,
    /// Parsed successfully.
    Loaded,
    /// The file exists but could not be parsed. Defaults are in use; the
    /// caller should warn, and `save` preserves the file rather than
    /// silently replacing it (see `save`).
    Corrupt(String),
}

/// A missing or corrupt config is not an error: the returned `Config` falls
/// back to defaults in both cases. The accompanying `ConfigStatus`
/// distinguishes which happened, since only one of them is destructive to
/// paper over silently.
pub fn load(path: &Path) -> (Config, ConfigStatus) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return (Config::default(), ConfigStatus::Missing),
    };

    match toml::from_str(&text) {
        Ok(cfg) => (cfg, ConfigStatus::Loaded),
        Err(e) => (Config::default(), ConfigStatus::Corrupt(e.to_string())),
    }
}

/// Builds the sibling path a corrupt `path` is renamed to before it would
/// otherwise be silently overwritten: `config.toml` becomes
/// `config.toml.invalid` in the same directory.
fn invalid_sibling_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");
    let invalid_name = format!("{file_name}.invalid");
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent.join(invalid_name),
        None => PathBuf::from(invalid_name),
    }
}

/// Writes `text` into `dest` only if the write and the following rename both
/// fully succeed. The bytes go to a temporary file in `dest`'s own directory
/// first — same-directory rename is atomic on Windows — and on any failure
/// the temp file is removed and `dest` is left completely untouched. Without
/// this, a crash or power loss mid-write would leave a truncated
/// `config.toml` that `load` would then read as `Corrupt`, discarding
/// whatever was there before the write started.
///
/// The temp filename combines a label, this process's id, and an atomically
/// incremented counter. `icon::convert::write_atomically` uses only the
/// process id, which collides across parallel test threads or two calls
/// within the same process; that gap is a known, already-logged shortcoming
/// there and is deliberately not repeated here.
fn write_atomically(dest: &Path, text: &str) -> Result<(), ConfigError> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{file_name}.{pid}.{n}.tmp");
    let tmp: PathBuf = match parent {
        Some(p) => p.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };

    if let Err(e) = std::fs::write(&tmp, text) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ConfigError::Io(e.to_string()));
    }

    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ConfigError::Io(e.to_string())
    })
}

/// Saves `cfg` to `path`. Two things protect the user's data along the way:
///
/// - If a file already exists at `path` and cannot be parsed as `Config`
///   (e.g. a hand-edit typo), it is renamed aside to a `.invalid` sibling
///   before anything is overwritten, so those bytes are never lost.
/// - The new content itself is written atomically (see `write_atomically`),
///   so a crash mid-write can never leave `path` truncated. This has to
///   compose with the rename-aside step above: doing the rename without the
///   atomic write would mean a crash during the write leaves the user with
///   *both* a `.invalid` file *and* a truncated `config.toml` — worse than
///   before the fix.
pub fn save(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    if let Ok(existing_text) = std::fs::read_to_string(path) {
        if toml::from_str::<Config>(&existing_text).is_err() {
            let invalid_path = invalid_sibling_path(path);
            std::fs::rename(path, &invalid_path).map_err(|e| ConfigError::Io(e.to_string()))?;
        }
    }

    let text = toml::to_string_pretty(cfg).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    write_atomically(path, &text)
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
        let (loaded, status) = load(&path);
        assert_eq!(loaded, c);
        assert_eq!(status, ConfigStatus::Loaded);

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
        let (loaded, _status) = load(&path);
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

        let (reloaded, _status) = load(&path);
        save(&path, &reloaded).unwrap();
        let second_bytes = std::fs::read_to_string(&path).unwrap();

        assert_eq!(first_bytes, second_bytes);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        let (cfg, status) =
            load(Path::new(r"C:\nope\wso_config_definitely_missing\config.toml"));
        assert_eq!(cfg, Config::default());
        assert_eq!(status, ConfigStatus::Missing);
    }

    #[test]
    fn a_corrupt_file_yields_defaults() {
        let dir = scratch_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        let (cfg, status) = load(&path);
        assert_eq!(cfg, Config::default());
        assert!(matches!(status, ConfigStatus::Corrupt(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `load`'s status must let a future caller tell "first run, nothing to
    /// lose" apart from "there was a file here and it broke" — the two
    /// situations look identical if `load` only ever returns a `Config`.
    #[test]
    fn load_reports_missing_for_an_absent_path_and_loaded_for_a_good_file() {
        let dir = scratch_dir("status_missing_vs_loaded");
        std::fs::create_dir_all(&dir).unwrap();
        let missing_path = dir.join("does-not-exist.toml");
        let (_, missing_status) = load(&missing_path);
        assert_eq!(missing_status, ConfigStatus::Missing);

        let good_path = dir.join("config.toml");
        save(&good_path, &Config::default()).unwrap();
        let (_, good_status) = load(&good_path);
        assert_eq!(good_status, ConfigStatus::Loaded);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The `Corrupt` status must carry the parse error, not just the fact
    /// that parsing failed, so a caller can show the user what is wrong
    /// with the file they hand-edited.
    #[test]
    fn corrupt_status_carries_a_non_empty_parse_error_message() {
        let dir = scratch_dir("status_corrupt_message");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();

        let (_, status) = load(&path);
        match status {
            ConfigStatus::Corrupt(message) => {
                assert!(!message.is_empty(), "parse error message must not be empty")
            }
            other => panic!("expected ConfigStatus::Corrupt, got {other:?}"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Overwriting a file the user broke by hand must not destroy it: the
    /// broken bytes are preserved as a sibling `.invalid` file so the user
    /// can see (and recover) what they had, instead of it vanishing the
    /// moment the tool next calls `save`.
    #[test]
    fn saving_over_an_unparseable_file_preserves_it_as_an_invalid_sibling() {
        let dir = scratch_dir("save_over_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let corrupt_bytes = "this is not toml {{{";
        std::fs::write(&path, corrupt_bytes).unwrap();

        save(&path, &Config::default()).unwrap();

        let invalid_path = dir.join("config.toml.invalid");
        assert_eq!(
            std::fs::read_to_string(&invalid_path).unwrap(),
            corrupt_bytes,
            "the original corrupt bytes must survive untouched in the .invalid sibling"
        );

        let (loaded, status) = load(&path);
        assert_eq!(loaded, Config::default());
        assert_eq!(
            status,
            ConfigStatus::Loaded,
            "the destination must now hold the freshly saved, valid config"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// If the destination's directory cannot be created (here, because a
    /// path component is a plain file rather than a directory — the same
    /// deterministic technique `icon::convert` uses to force this failure
    /// without needing a real disk-full or permission-denied condition),
    /// `save` must fail cleanly and must not leave a temporary file behind
    /// anywhere it could later be mistaken for real output.
    #[test]
    fn save_into_an_uncreatable_directory_fails_without_leaving_a_stray_temp_file() {
        let dir = scratch_dir("uncreatable_dir");
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker.txt");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let path = blocker.join("subdir").join("config.toml");

        let result = save(&path, &Config::default());
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "only the blocker file should remain; no stray temp file"
        );

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
