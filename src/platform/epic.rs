use super::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One entry from the legacy Epic Games Launcher's manifest directory
/// (`C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests\*.item`). Unknown
/// fields are ignored so a launcher update that adds keys does not break
/// parsing.
#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    #[serde(rename = "DisplayName")]
    pub display_name: String,
    #[serde(rename = "InstallLocation")]
    pub install_location: String,
    #[serde(rename = "LaunchExecutable")]
    pub launch_executable: String,
    #[serde(rename = "AppName")]
    pub app_name: String,
    #[serde(rename = "CatalogNamespace")]
    pub catalog_namespace: String,
    #[serde(rename = "CatalogItemId")]
    pub catalog_item_id: String,
    #[serde(rename = "AppCategories", default)]
    pub app_categories: Vec<String>,
    #[serde(rename = "bIsIncompleteInstall", default)]
    pub incomplete: bool,
}

impl Manifest {
    /// Keeps only playable, fully installed titles. This is what excludes
    /// DLC, plugins and Unreal Engine installs.
    pub(crate) fn is_installed_game(&self) -> bool {
        !self.incomplete
            && !self.launch_executable.is_empty()
            && self
                .app_categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case("games"))
    }
}

/// One record from the Epic Games Launcher Preview (`eglv2`) transaction log
/// (`%LOCALAPPDATA%\com.epicgames.eglv2\installations.jsonl`).
///
/// `installations.jsonl` is an append-only JSON Lines log: line 1 is a header
/// (`{"version":1,"writtenAt":...,"entries":...}`), and each subsequent line
/// records the state of an installation after an install, update, pause, or
/// removal. The latest line for a given `installation_id` is its current state.
///
/// Notice what is true when an update is paused or queued for an already
/// installed game: `complete` drops to `false` while `version` still holds the
/// installed build and `pending_version` holds the incoming one (a fresh
/// download that has never finished has `complete: false` and no `version` at
/// all). Treating `complete: false` alone as "not installed" would hide a
/// playable game with a real executable on disk the moment an update is
/// pending.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PreviewLogEntry {
    #[serde(rename = "installationId")]
    pub installation_id: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(rename = "itemId", default)]
    pub item_id: String,
    #[serde(rename = "artifactId", default)]
    pub artifact_id: String,
    #[serde(rename = "parentInstallationId", default)]
    pub parent_installation_id: Option<String>,
    #[serde(rename = "isFab", default)]
    pub is_fab: bool,
    #[serde(rename = "launchExecutable", default)]
    pub launch_executable: String,
}

impl PreviewLogEntry {
    /// Whether this record describes a standalone game installation that has
    /// completed at least once and has not been removed.
    pub(crate) fn is_installed(&self) -> bool {
        !self.deleted
            && !self.is_fab
            && self
                .parent_installation_id
                .as_deref()
                .unwrap_or("")
                .is_empty()
            && (self.complete || self.version.as_deref().is_some_and(|v| !v.is_empty()))
            && !self.dir.is_empty()
            && !self.launch_executable.is_empty()
            && !self.namespace.is_empty()
            && !self.item_id.is_empty()
            && !self.artifact_id.is_empty()
    }
}

/// Metadata resolved from `eglv2`'s local catalog and library caches so an
/// installation record (which carries only IDs and paths) gets its human
/// display title and category filtering.
#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogMeta {
    pub title: Option<String>,
    pub is_non_game: bool,
}

#[derive(Deserialize)]
struct CatalogCacheFile {
    #[serde(default)]
    items: HashMap<String, CatalogCacheEnvelope>,
}

#[derive(Deserialize)]
struct CatalogCacheEnvelope {
    item: Option<CatalogCacheItem>,
}

#[derive(Deserialize)]
struct CatalogCacheItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    categories: Vec<CatalogCategory>,
    #[serde(rename = "mainGameItem", default)]
    main_game_item: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CatalogCategory {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct LibraryCacheFile {
    #[serde(default)]
    items: Vec<LibraryCacheItem>,
}

#[derive(Deserialize)]
struct LibraryCacheItem {
    #[serde(rename = "appId")]
    app_id: Option<LibraryAppId>,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
struct LibraryAppId {
    #[serde(rename = "itemId", default)]
    item_id: String,
    #[serde(rename = "appName", default)]
    app_name: String,
}

/// A `.lnk` cannot target a protocol URI, so the shortcut runs Explorer and
/// hands it the URI. Launching through the URI rather than the executable
/// preserves Epic's own launch handling and entitlement checks.
pub fn launch_uri(namespace: &str, catalog_item_id: &str, app_name: &str) -> String {
    format!(
        "com.epicgames.launcher://apps/{namespace}%3A{catalog_item_id}%3A{app_name}?action=launch&silent=true"
    )
}

pub struct EpicProvider {
    /// Legacy Epic Games Launcher manifest directory
    /// (`C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests`), if present.
    manifests_dir: Option<PathBuf>,
    /// Epic Games Launcher Preview (`eglv2`) data root
    /// (`%LOCALAPPDATA%\com.epicgames.eglv2`), if present.
    preview_data_dir: Option<PathBuf>,
    launcher_exe: PathBuf,
}

impl EpicProvider {
    /// Detects an installed Epic Games Launcher — either the classic client
    /// storing `.item` manifests under `C:\ProgramData\Epic\EpicGamesLauncher`,
    /// the Preview (`eglv2`) client storing `installations.jsonl` under
    /// `%LOCALAPPDATA%\com.epicgames.eglv2`, or both during a transition.
    pub fn detect() -> Option<EpicProvider> {
        let legacy_manifests =
            PathBuf::from(r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests");
        let preview_data = preview_data_candidates()
            .into_iter()
            .find(|d| d.join("installations.jsonl").is_file());

        let manifests_dir = legacy_manifests.is_dir().then_some(legacy_manifests);

        if manifests_dir.is_none() && preview_data.is_none() {
            return None;
        }

        Some(EpicProvider {
            manifests_dir,
            preview_data_dir: preview_data,
            launcher_exe: detect_launcher_exe(),
        })
    }

    pub fn from_dirs(manifests_dir: PathBuf, launcher_exe: PathBuf) -> EpicProvider {
        EpicProvider {
            manifests_dir: Some(manifests_dir),
            preview_data_dir: None,
            launcher_exe,
        }
    }

    pub fn from_preview_dir(preview_data_dir: PathBuf, launcher_exe: PathBuf) -> EpicProvider {
        EpicProvider {
            manifests_dir: None,
            preview_data_dir: Some(preview_data_dir),
            launcher_exe,
        }
    }

    pub fn from_both_dirs(
        manifests_dir: PathBuf,
        preview_data_dir: PathBuf,
        launcher_exe: PathBuf,
    ) -> EpicProvider {
        EpicProvider {
            manifests_dir: Some(manifests_dir),
            preview_data_dir: Some(preview_data_dir),
            launcher_exe,
        }
    }

    fn explorer() -> PathBuf {
        std::env::var("WINDIR")
            .map(|w| PathBuf::from(w).join("explorer.exe"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\explorer.exe"))
    }

    fn build_game(
        &self,
        id: String,
        name: String,
        namespace: &str,
        catalog_item_id: &str,
        install_dir: PathBuf,
        launch_executable: &str,
    ) -> Game {
        let exe = install_dir.join(launch_executable);
        Game {
            platform: Platform::Epic,
            launch: Launch {
                program: Self::explorer(),
                args: format!("\"{}\"", launch_uri(namespace, catalog_item_id, &id)),
                working_dir: None,
            },
            // Epic declares its executable, so no heuristic is needed here.
            icon_candidates: vec![
                IconCandidate::Executable(exe),
                IconCandidate::Fallback(self.launcher_exe.clone()),
            ],
            id,
            name,
            install_dir,
        }
    }

    fn read_legacy_manifests(&self, dir: &Path) -> Result<Vec<Game>, ProviderError> {
        let entries = std::fs::read_dir(dir).map_err(|e| ProviderError::Unreadable {
            path: dir.to_path_buf(),
            detail: e.to_string(),
        })?;

        let mut games = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            // Case-insensitively: matching strictly made a `Fortnite.ITEM`
            // manifest invisible, so the game was simply never detected —
            // no error, and nothing to explain the absence.
            if !crate::paths::has_extension(&path, "item") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(m) = serde_json::from_str::<Manifest>(&text) else {
                continue;
            };
            if !m.is_installed_game() {
                continue;
            }

            games.push(self.build_game(
                m.app_name,
                m.display_name,
                &m.catalog_namespace,
                &m.catalog_item_id,
                PathBuf::from(&m.install_location),
                &m.launch_executable,
            ));
        }
        Ok(games)
    }

    fn read_preview_installations(&self, data_dir: &Path) -> Result<Vec<Game>, ProviderError> {
        let jsonl_path = data_dir.join("installations.jsonl");
        let text = std::fs::read_to_string(&jsonl_path).map_err(|e| ProviderError::Unreadable {
            path: jsonl_path,
            detail: e.to_string(),
        })?;

        let entries = parse_preview_installations(&text);
        let meta = load_preview_metadata(data_dir);

        let mut games = Vec::new();
        for entry in entries {
            let install_dir = PathBuf::from(&entry.dir);
            // Unlike legacy `.item` files (which are removed on uninstall),
            // `installations.jsonl` is a transaction log and `eglv2` drops
            // stale records on startup when their directory or executable no
            // longer exists on disk. Checking the install directory here
            // avoids creating shortcuts for a folder deleted while the
            // launcher was closed.
            if !install_dir.is_dir() {
                continue;
            }

            let item_meta = meta
                .get(&entry.item_id)
                .or_else(|| meta.get(&entry.artifact_id));
            if item_meta.is_some_and(|m| m.is_non_game) {
                continue;
            }

            let name = item_meta
                .and_then(|m| m.title.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| fallback_display_name(&install_dir, &entry.artifact_id));

            games.push(self.build_game(
                entry.artifact_id,
                name,
                &entry.namespace,
                &entry.item_id,
                install_dir,
                &entry.launch_executable,
            ));
        }
        Ok(games)
    }
}

/// Replays an `eglv2` `installations.jsonl` log and returns the active,
/// installed game entries in deterministic order.
///
/// Each line with an `installationId` updates or tombstones that installation;
/// the header line (and any malformed line) has no `installationId` and is
/// skipped.
pub(crate) fn parse_preview_installations(text: &str) -> Vec<PreviewLogEntry> {
    let mut by_id: HashMap<String, PreviewLogEntry> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<PreviewLogEntry>(line) else {
            continue;
        };
        if entry.installation_id.is_empty() {
            continue;
        }
        if !by_id.contains_key(&entry.installation_id) {
            order.push(entry.installation_id.clone());
        }
        by_id.insert(entry.installation_id.clone(), entry);
    }

    order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .filter(|e| e.is_installed())
        .collect()
}

/// Collects titles and category flags from `eglv2`'s local caches:
/// - `cache/<env>/catalog-cache.json` (authoritative categories + titles)
/// - `cache/<env>/account/<id>/library-cache.json` (titles for owned apps)
/// - `import-scan/v1-items/*.item` (legacy manifests migrated on first run)
fn load_preview_metadata(data_dir: &Path) -> HashMap<String, CatalogMeta> {
    let mut out: HashMap<String, CatalogMeta> = HashMap::new();

    // 1. Imported v1 `.item` manifests (lowest priority, overwritten by live catalog).
    let v1_items = data_dir.join("import-scan").join("v1-items");
    if let Ok(entries) = std::fs::read_dir(v1_items) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !crate::paths::has_extension(&path, "item") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(m) = serde_json::from_str::<Manifest>(&text) else {
                continue;
            };
            let is_game = m
                .app_categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case("games"));
            let meta = CatalogMeta {
                title: (!m.display_name.is_empty()).then_some(m.display_name),
                is_non_game: !m.app_categories.is_empty() && !is_game,
            };
            if !m.catalog_item_id.is_empty() {
                out.insert(m.catalog_item_id, meta.clone());
            }
            if !m.app_name.is_empty() {
                out.insert(m.app_name, meta);
            }
        }
    }

    // 2. Per-environment caches under `cache/*/` (typically `cache/prod/`).
    let cache_root = data_dir.join("cache");
    if let Ok(env_dirs) = std::fs::read_dir(&cache_root) {
        for env_entry in env_dirs.flatten() {
            let env_path = env_entry.path();
            if !env_path.is_dir() {
                continue;
            }

            // Library cache under `cache/<env>/account/*/library-cache.json`.
            let account_root = env_path.join("account");
            if let Ok(accounts) = std::fs::read_dir(&account_root) {
                for acc in accounts.flatten() {
                    let lib_path = acc.path().join("library-cache.json");
                    if let Ok(text) = std::fs::read_to_string(&lib_path) {
                        merge_library_cache(&text, &mut out);
                    }
                }
            }

            // Catalog cache under `cache/<env>/catalog-cache.json`.
            let catalog_path = env_path.join("catalog-cache.json");
            if let Ok(text) = std::fs::read_to_string(&catalog_path) {
                merge_catalog_cache(&text, &mut out);
            }
        }
    }

    out
}

pub(crate) fn merge_library_cache(text: &str, out: &mut HashMap<String, CatalogMeta>) {
    let Ok(parsed) = serde_json::from_str::<LibraryCacheFile>(text) else {
        return;
    };
    for item in parsed.items {
        if item.title.is_empty() {
            continue;
        }
        let Some(app_id) = item.app_id else { continue };
        for key in [app_id.item_id, app_id.app_name] {
            if !key.is_empty() {
                let entry = out.entry(key).or_default();
                if entry.title.is_none() {
                    entry.title = Some(item.title.clone());
                }
            }
        }
    }
}

pub(crate) fn merge_catalog_cache(text: &str, out: &mut HashMap<String, CatalogMeta>) {
    let Ok(parsed) = serde_json::from_str::<CatalogCacheFile>(text) else {
        return;
    };
    for (map_key, env) in parsed.items {
        let Some(item) = env.item else { continue };
        let has_games = item.categories.iter().any(|c| {
            c.path.eq_ignore_ascii_case("games")
                || c.path
                    .split('/')
                    .any(|seg| seg.eq_ignore_ascii_case("games"))
        });
        let is_addon = item.main_game_item.is_some();
        let is_non_game = is_addon || (!item.categories.is_empty() && !has_games);

        for key in [map_key, item.id] {
            if !key.is_empty() {
                let entry = out.entry(key).or_default();
                if !item.title.is_empty() {
                    entry.title = Some(item.title.clone());
                }
                entry.is_non_game = is_non_game;
            }
        }
    }
}

fn fallback_display_name(install_dir: &Path, artifact_id: &str) -> String {
    install_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(artifact_id)
        .to_string()
}

fn preview_data_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in registry_preview_appdata_paths() {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let candidate = PathBuf::from(local).join("com.epicgames.eglv2");
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

fn detect_launcher_exe() -> PathBuf {
    for candidate in launcher_exe_candidates() {
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(
        r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win32\EpicGamesLauncher.exe",
    )
}

fn launcher_exe_candidates() -> Vec<PathBuf> {
    let mut out = registry_launcher_exes();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        out.push(
            PathBuf::from(local)
                .join("Epic Games")
                .join("Launcher")
                .join("EpicGamesLauncher.exe"),
        );
    }
    out.push(PathBuf::from(
        r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe",
    ));
    out.push(PathBuf::from(
        r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win32\EpicGamesLauncher.exe",
    ));
    out.push(PathBuf::from(
        r"C:\Program Files\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe",
    ));
    out
}

#[cfg(windows)]
fn registry_preview_appdata_paths() -> Vec<PathBuf> {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let mut out = Vec::new();
    let queries = [
        (
            HKEY_CURRENT_USER,
            w!("Software\\Epic Games\\EpicGamesLauncher"),
            w!("AppDataPath"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\WOW6432Node\\Epic Games\\EpicGamesLauncher"),
            w!("AppDataPath"),
        ),
    ];

    for (hkey, subkey, value) in queries {
        let mut buf = [0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                hkey,
                subkey,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut size),
            )
        };
        if status.is_ok() {
            let len = (size as usize / 2).saturating_sub(1);
            let s = String::from_utf16_lossy(&buf[..len]);
            let path = PathBuf::from(&s);
            // `eglv2` writes its own `com.epicgames.eglv2` path here, whereas
            // the legacy launcher writes `...\EpicGamesLauncher\Data\Manifests`.
            if !s.is_empty() && path.join("installations.jsonl").is_file() {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_preview_appdata_paths() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(windows)]
fn registry_launcher_exes() -> Vec<PathBuf> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut out = Vec::new();
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\EpicGames\\Unreal Engine"),
            w!("INSTALLDIR"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_ok() {
        let len = (size as usize / 2).saturating_sub(1);
        let s = String::from_utf16_lossy(&buf[..len]);
        if !s.is_empty() {
            out.push(PathBuf::from(s).join("EpicGamesLauncher.exe"));
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_launcher_exes() -> Vec<PathBuf> {
    Vec::new()
}

impl GameProvider for EpicProvider {
    fn platform(&self) -> Platform {
        Platform::Epic
    }

    fn games(&self) -> Result<Vec<Game>, ProviderError> {
        let mut games = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut first_err: Option<ProviderError> = None;
        let mut any_succeeded = false;

        // Prefer Preview (`eglv2`) when both are present on disk so an active
        // Preview installation wins over stale v1 manifests left behind before
        // v1 removal settled.
        if let Some(preview_dir) = &self.preview_data_dir {
            match self.read_preview_installations(preview_dir) {
                Ok(preview_games) => {
                    any_succeeded = true;
                    for g in preview_games {
                        if seen_ids.insert(g.id.clone()) {
                            games.push(g);
                        }
                    }
                }
                Err(e) => first_err = Some(e),
            }
        }

        if let Some(manifests_dir) = &self.manifests_dir {
            match self.read_legacy_manifests(manifests_dir) {
                Ok(legacy_games) => {
                    any_succeeded = true;
                    for g in legacy_games {
                        if seen_ids.insert(g.id.clone()) {
                            games.push(g);
                        }
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }

        if !any_succeeded
            && let Some(err) = first_err {
                return Err(err);
            }

        games.sort_by_key(|g| g.name.to_lowercase());
        Ok(games)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME_ITEM: &str = r#"{
        "DisplayName": "Fall Guys",
        "InstallLocation": "D:\\Epic\\FallGuys",
        "LaunchExecutable": "FallGuys_client.exe",
        "AppName": "9c88",
        "CatalogNamespace": "0a2d9f",
        "CatalogItemId": "abc123",
        "AppCategories": ["public", "games", "applications"],
        "bIsIncompleteInstall": false
    }"#;

    const DLC_ITEM: &str = r#"{
        "DisplayName": "Some Addon",
        "InstallLocation": "D:\\Epic\\Addon",
        "LaunchExecutable": "addon.exe",
        "AppName": "addon1",
        "CatalogNamespace": "ns",
        "CatalogItemId": "cid",
        "AppCategories": ["public", "addons"],
        "bIsIncompleteInstall": false
    }"#;

    const INCOMPLETE_ITEM: &str = r#"{
        "DisplayName": "Half Downloaded",
        "InstallLocation": "D:\\Epic\\Half",
        "LaunchExecutable": "half.exe",
        "AppName": "half1",
        "CatalogNamespace": "ns",
        "CatalogItemId": "cid",
        "AppCategories": ["games"],
        "bIsIncompleteInstall": true
    }"#;

    const EMPTY_EXECUTABLE_ITEM: &str = r#"{
        "DisplayName": "No Executable",
        "InstallLocation": "D:\\Epic\\NoExe",
        "LaunchExecutable": "",
        "AppName": "noexe1",
        "CatalogNamespace": "ns",
        "CatalogItemId": "cid",
        "AppCategories": ["games"],
        "bIsIncompleteInstall": false
    }"#;

    #[test]
    fn accepts_a_game_manifest() {
        let m: Manifest = serde_json::from_str(GAME_ITEM).unwrap();
        assert!(m.is_installed_game());
        assert_eq!(m.display_name, "Fall Guys");
    }

    #[test]
    fn rejects_non_game_categories() {
        let m: Manifest = serde_json::from_str(DLC_ITEM).unwrap();
        assert!(!m.is_installed_game());
    }

    #[test]
    fn rejects_incomplete_installs() {
        let m: Manifest = serde_json::from_str(INCOMPLETE_ITEM).unwrap();
        assert!(!m.is_installed_game());
    }

    #[test]
    fn rejects_empty_launch_executable() {
        let m: Manifest = serde_json::from_str(EMPTY_EXECUTABLE_ITEM).unwrap();
        assert!(!m.is_installed_game());
    }

    #[test]
    fn tolerates_unknown_fields() {
        let json = r#"{
            "DisplayName": "X", "InstallLocation": "C:\\X",
            "LaunchExecutable": "x.exe", "AppName": "x",
            "CatalogNamespace": "n", "CatalogItemId": "c",
            "AppCategories": ["games"], "SomethingEpicAddedLater": 42
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.is_installed_game());
    }

    #[test]
    fn builds_the_launch_uri_with_encoded_separators() {
        assert_eq!(
            launch_uri("0a2d9f", "abc123", "9c88"),
            "com.epicgames.launcher://apps/0a2d9f%3Aabc123%3A9c88?action=launch&silent=true"
        );
    }

    #[test]
    fn preview_log_replays_latest_state_per_installation_id() {
        let log = r#"{"version":1,"writtenAt":"2026-11-25T02:03:09Z","entries":4}
{"installationId":"ID1","complete":false,"dir":"D:\\Epic Games\\BloonsTD6","platform":"Windows","namespace":"6a8d","itemId":"49c4","artifactId":"7786","pendingVersion":"57.4","launchExecutable":"BloonsTD6.exe","writtenAt":"2026-11-21T04:55:07Z"}
{"installationId":"ID1","complete":true,"dir":"D:\\Epic Games\\BloonsTD6","platform":"Windows","namespace":"6a8d","itemId":"49c4","artifactId":"7786","version":"57.4","launchExecutable":"BloonsTD6.exe","writtenAt":"2026-11-21T04:56:31Z"}
{"installationId":"ID2","complete":true,"dir":"D:\\Epic Games\\Fortnite","platform":"Windows","namespace":"fn","itemId":"4fe7","artifactId":"Fortnite","version":"43.10","launchExecutable":"FortniteGame/Binaries/Win64/FortniteBootstrapper.exe","writtenAt":"2026-11-14T19:09:48Z"}
{"installationId":"ID2","complete":false,"dir":"D:\\Epic Games\\Fortnite","platform":"Windows","namespace":"fn","itemId":"4fe7","artifactId":"Fortnite","version":"43.10","pendingVersion":"43.11","launchExecutable":"FortniteGame/Binaries/Win64/FortniteBootstrapper.exe","writtenAt":"2026-11-25T02:02:47Z"}
"#;
        let parsed = parse_preview_installations(log);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].artifact_id, "7786");
        assert!(parsed[0].complete);
        // Fortnite has complete=false because an update is pending, but has an installed `version`
        assert_eq!(parsed[1].artifact_id, "Fortnite");
        assert!(!parsed[1].complete);
        assert_eq!(parsed[1].version.as_deref(), Some("43.10"));
    }

    #[test]
    fn preview_log_excludes_fresh_incomplete_deleted_and_parented_addons() {
        let log = r#"{"version":1,"writtenAt":"2026-11-25T02:03:09Z","entries":4}
{"installationId":"FRESH_HALF","complete":false,"dir":"D:\\Epic\\Half","namespace":"ns","itemId":"i1","artifactId":"a1","pendingVersion":"1.0","launchExecutable":"half.exe"}
{"installationId":"DELETED_GAME","complete":true,"dir":"D:\\Epic\\Gone","namespace":"ns","itemId":"i2","artifactId":"a2","version":"1.0","launchExecutable":"gone.exe"}
{"installationId":"DELETED_GAME","deleted":true,"writtenAt":"2026-11-25T02:03:00Z"}
{"installationId":"ADDON","complete":true,"dir":"D:\\Epic\\Addon","namespace":"ns","itemId":"i3","artifactId":"a3","version":"1.0","parentInstallationId":"PARENT1","launchExecutable":"addon.exe"}
{"installationId":"FAB_ASSET","complete":true,"dir":"D:\\Epic\\Fab","namespace":"ns","itemId":"i4","artifactId":"a4","version":"1.0","isFab":true,"launchExecutable":"fab.exe"}
"#;
        let parsed = parse_preview_installations(log);
        assert!(
            parsed.is_empty(),
            "expected no playable games, got {parsed:?}"
        );
    }

    #[test]
    fn catalog_and_library_caches_resolve_titles_and_filter_non_games() {
        let mut meta = HashMap::new();
        let library_json = r#"{
            "items": [
                {
                    "appId": {"namespaceId": "6a8d", "itemId": "49c4", "appName": "7786"},
                    "title": "Bloons TD 6"
                }
            ]
        }"#;
        let catalog_json = r#"{
            "items": {
                "ue_item": {
                    "item": {
                        "id": "ue_item",
                        "title": "Unreal Engine",
                        "categories": [{"path": "engines/ue5"}, {"path": "engines"}]
                    }
                },
                "dlc_item": {
                    "item": {
                        "id": "dlc_item",
                        "title": "Some DLC",
                        "categories": [{"path": "games"}, {"path": "applications"}],
                        "mainGameItem": {"id": "parent_game"}
                    }
                },
                "gta_item": {
                    "item": {
                        "id": "gta_item",
                        "title": "Grand Theft Auto V Enhanced",
                        "categories": [{"path": "games"}, {"path": "applications"}],
                        "mainGameItem": null
                    }
                }
            }
        }"#;

        merge_library_cache(library_json, &mut meta);
        merge_catalog_cache(catalog_json, &mut meta);

        assert_eq!(
            meta.get("49c4").and_then(|m| m.title.as_deref()),
            Some("Bloons TD 6")
        );
        assert!(!meta.get("49c4").unwrap().is_non_game);

        assert_eq!(
            meta.get("gta_item").and_then(|m| m.title.as_deref()),
            Some("Grand Theft Auto V Enhanced")
        );
        assert!(!meta.get("gta_item").unwrap().is_non_game);

        assert!(meta.get("ue_item").unwrap().is_non_game);
        assert!(meta.get("dlc_item").unwrap().is_non_game);
    }
}
