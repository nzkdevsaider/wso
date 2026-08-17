use super::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError};
use serde::Deserialize;
use std::path::PathBuf;

/// One entry from Epic's manifest directory. Unknown fields are ignored so a
/// launcher update that adds keys does not break parsing.
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

/// A `.lnk` cannot target a protocol URI, so the shortcut runs Explorer and
/// hands it the URI. Launching through the URI rather than the executable
/// preserves Epic's own launch handling and entitlement checks.
pub fn launch_uri(namespace: &str, catalog_item_id: &str, app_name: &str) -> String {
    format!(
        "com.epicgames.launcher://apps/{namespace}%3A{catalog_item_id}%3A{app_name}?action=launch&silent=true"
    )
}

pub struct EpicProvider {
    manifests_dir: PathBuf,
    launcher_exe: PathBuf,
}

impl EpicProvider {
    pub fn detect() -> Option<EpicProvider> {
        let manifests = PathBuf::from(r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests");
        if !manifests.is_dir() {
            return None;
        }
        Some(EpicProvider {
            manifests_dir: manifests,
            launcher_exe: PathBuf::from(
                r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win32\EpicGamesLauncher.exe",
            ),
        })
    }

    pub fn from_dirs(manifests_dir: PathBuf, launcher_exe: PathBuf) -> EpicProvider {
        EpicProvider {
            manifests_dir,
            launcher_exe,
        }
    }

    fn explorer() -> PathBuf {
        std::env::var("WINDIR")
            .map(|w| PathBuf::from(w).join("explorer.exe"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\explorer.exe"))
    }
}

impl GameProvider for EpicProvider {
    fn platform(&self) -> Platform {
        Platform::Epic
    }

    fn games(&self) -> Result<Vec<Game>, ProviderError> {
        let entries =
            std::fs::read_dir(&self.manifests_dir).map_err(|e| ProviderError::Unreadable {
                path: self.manifests_dir.clone(),
                detail: e.to_string(),
            })?;

        let mut games = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("item") {
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

            let install_dir = PathBuf::from(&m.install_location);
            let exe = install_dir.join(&m.launch_executable);

            games.push(Game {
                platform: Platform::Epic,
                id: m.app_name.clone(),
                name: m.display_name.clone(),
                launch: Launch {
                    program: Self::explorer(),
                    args: format!(
                        "\"{}\"",
                        launch_uri(&m.catalog_namespace, &m.catalog_item_id, &m.app_name)
                    ),
                    working_dir: None,
                },
                // Epic declares its executable, so no heuristic is needed here.
                icon_candidates: vec![
                    IconCandidate::Executable(exe),
                    IconCandidate::Fallback(self.launcher_exe.clone()),
                ],
                install_dir,
            });
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
}
