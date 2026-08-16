use super::vdf;
use super::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError};
use crate::icon::exe_finder;
use std::path::{Path, PathBuf};

/// Appids that ship with Steam but are not games.
const INFRA_APPIDS: [&str; 6] =
    ["228980", "1070560", "1391110", "1493710", "1580130", "1826330"];

/// Name prefixes that identify runtimes and compatibility tools.
const INFRA_NAME_PREFIXES: [&str; 3] =
    ["proton ", "steam linux runtime", "steamworks "];

pub struct SteamProvider {
    root: PathBuf,
}

pub(crate) struct AppManifest {
    pub appid: String,
    pub name: String,
    pub installdir: String,
}

impl SteamProvider {
    /// Resolves Steam's install directory. The registry is tried first, but
    /// the conventional paths are a required fallback, not a nicety: on the
    /// machine this was validated against, `HKCU\Software\Valve\Steam` did not
    /// answer while Steam was correctly installed.
    pub fn detect() -> Option<SteamProvider> {
        for candidate in registry_roots().into_iter().chain(conventional_roots()) {
            if candidate.join("steam.exe").is_file() {
                return Some(SteamProvider { root: candidate });
            }
        }
        None
    }

    pub fn from_root(root: PathBuf) -> SteamProvider {
        SteamProvider { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn launch_for(&self, appid: &str) -> Launch {
        Launch {
            program: self.root.join("steam.exe"),
            args: format!("-applaunch {appid}"),
            working_dir: Some(self.root.clone()),
        }
    }

    pub fn libraries(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let path = self.root.join("steamapps").join("libraryfolders.vdf");
        let text = std::fs::read_to_string(&path).map_err(|e| ProviderError::Unreadable {
            path: path.clone(),
            detail: e.to_string(),
        })?;
        parse_library_paths(&text).ok_or(ProviderError::Unreadable {
            path,
            detail: "no library paths found".into(),
        })
    }
}

pub(crate) fn parse_library_paths(text: &str) -> Option<Vec<PathBuf>> {
    let root = vdf::parse(text).ok()?;
    let folders = root.get("libraryfolders")?;
    let paths: Vec<PathBuf> = folders
        .entries()
        .iter()
        .filter_map(|(_, v)| v.get("path")?.as_str().map(PathBuf::from))
        .collect();
    if paths.is_empty() {
        None
    } else {
        Some(paths)
    }
}

pub(crate) fn parse_app_manifest(text: &str) -> Option<AppManifest> {
    let root = vdf::parse(text).ok()?;
    let state = root.get("AppState")?;
    Some(AppManifest {
        appid: state.get("appid")?.as_str()?.to_string(),
        name: state.get("name")?.as_str()?.to_string(),
        installdir: state.get("installdir")?.as_str()?.to_string(),
    })
}

pub(crate) fn is_infrastructure(appid: &str, name: &str) -> bool {
    if INFRA_APPIDS.contains(&appid) {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    INFRA_NAME_PREFIXES.iter().any(|p| lower.starts_with(p))
}

fn conventional_roots() -> Vec<PathBuf> {
    let mut out = vec![
        PathBuf::from(r"C:\Program Files (x86)\Steam"),
        PathBuf::from(r"C:\Program Files\Steam"),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        out.push(PathBuf::from(local).join("Steam"));
    }
    out
}

impl GameProvider for SteamProvider {
    fn platform(&self) -> Platform {
        Platform::Steam
    }

    fn games(&self) -> Result<Vec<Game>, ProviderError> {
        let mut games = Vec::new();
        for library in self.libraries()? {
            let steamapps = library.join("steamapps");
            let Ok(entries) = std::fs::read_dir(&steamapps) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_manifest = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf"));
                if !is_manifest {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Some(app) = parse_app_manifest(&text) else {
                    continue;
                };
                if is_infrastructure(&app.appid, &app.name) {
                    continue;
                }
                let install_dir = steamapps.join("common").join(&app.installdir);
                if !install_dir.is_dir() {
                    continue;
                }
                games.push(self.build_game(app, install_dir));
            }
        }
        games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(games)
    }
}

impl SteamProvider {
    fn build_game(&self, app: AppManifest, install_dir: PathBuf) -> Game {
        let candidates = exe_finder::collect_candidates(&install_dir, 4);
        let mut icons = Vec::new();
        if let Some(exe) = exe_finder::pick_executable(&app.name, &app.installdir, &candidates) {
            icons.push(IconCandidate::Executable(install_dir.join(&exe.rel_path)));
        }
        icons.push(IconCandidate::ImageFile(
            self.root
                .join("appcache")
                .join("librarycache")
                .join(&app.appid)
                .join("logo.png"),
        ));
        icons.push(IconCandidate::Fallback(self.root.join("steam.exe")));

        Game {
            platform: Platform::Steam,
            launch: self.launch_for(&app.appid),
            id: app.appid,
            name: app.name,
            install_dir,
            icon_candidates: icons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_paths() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}
"#;
        let libs = parse_library_paths(vdf).unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[1], PathBuf::from(r"D:\SteamLibrary"));
    }

    #[test]
    fn parses_an_app_manifest() {
        let acf = r#"
"AppState"
{
	"appid"		"105600"
	"name"		"Terraria"
	"installdir"		"Terraria"
}
"#;
        let app = parse_app_manifest(acf).unwrap();
        assert_eq!(app.appid, "105600");
        assert_eq!(app.name, "Terraria");
        assert_eq!(app.installdir, "Terraria");
    }

    #[test]
    fn rejects_a_manifest_missing_fields() {
        assert!(parse_app_manifest("\"AppState\"\n{\n\"appid\" \"1\"\n}\n").is_none());
    }

    #[test]
    fn filters_infrastructure_by_appid() {
        assert!(is_infrastructure("228980", "Steamworks Common Redistributables"));
        assert!(is_infrastructure("1070560", "Steam Linux Runtime"));
        assert!(!is_infrastructure("105600", "Terraria"));
    }

    #[test]
    fn filters_infrastructure_by_name() {
        assert!(is_infrastructure("999999", "Proton 9.0"));
        assert!(is_infrastructure("999998", "Steam Linux Runtime 3.0 (sniper)"));
        assert!(is_infrastructure("999997", "Steamworks Shared"));
        assert!(!is_infrastructure("999996", "Protonwall"));
    }

    #[test]
    fn builds_a_launch_command() {
        let p = SteamProvider::from_root(PathBuf::from(r"C:\Steam"));
        let launch = p.launch_for("440");
        assert_eq!(launch.program, PathBuf::from(r"C:\Steam\steam.exe"));
        assert_eq!(launch.args, "-applaunch 440");
    }
}

#[cfg(windows)]
fn registry_roots() -> Vec<PathBuf> {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let mut out = Vec::new();
    let queries = [
        (HKEY_CURRENT_USER, w!("Software\\Valve\\Steam"), w!("SteamPath")),
        (
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\WOW6432Node\\Valve\\Steam"),
            w!("InstallPath"),
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
            if !s.is_empty() {
                out.push(PathBuf::from(s));
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_roots() -> Vec<PathBuf> {
    Vec::new()
}
