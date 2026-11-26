use super::vdf;
use super::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError};
use crate::icon::exe_finder;
use std::path::{Path, PathBuf};

/// Appids that ship with Steam but are not games.
const INFRA_APPIDS: [&str; 6] = [
    "228980", "1070560", "1391110", "1493710", "1580130", "1826330",
];

/// Name prefixes that identify runtimes and compatibility tools.
const INFRA_NAME_PREFIXES: [&str; 3] = ["proton ", "steam linux runtime", "steamworks "];

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
    if paths.is_empty() { None } else { Some(paths) }
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
        games.sort_by_key(|g| g.name.to_lowercase());
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
        let cache_dir = self
            .root
            .join("appcache")
            .join("librarycache")
            .join(&app.appid);
        if let Some(image) = pick_library_image(&cache_dir) {
            icons.push(IconCandidate::ImageFile(image));
        }
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

/// Picks an image file from a game's Steam library-cache directory.
///
/// Recent Steam clients name most assets after a content hash, often with no
/// extension at all, so the well-known filenames are tried first and the
/// largest remaining regular file is the last resort. Returns `None` — never
/// a path that hasn't been confirmed to exist — when the directory is
/// missing or contains no regular files, so callers can fall through to
/// their own fallback instead of pointing at nothing.
///
/// "Largest" alone is not a rule: two assets in one library-cache directory
/// can be exactly the same size, and then the answer would come down to
/// whatever order `read_dir` happened to return them in — which is not
/// guaranteed, so the same game could show one icon today and a different one
/// tomorrow with nothing having changed. Equal sizes are settled by filename
/// so the choice is the user's to keep.
pub(crate) fn pick_library_image(dir: &Path) -> Option<PathBuf> {
    const PREFERRED: [&str; 2] = ["logo.png", "library_600x900.jpg"];
    for name in PREFERRED {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| (entry.path(), meta.len()))
        })
        .collect();

    // Largest first; ties broken by filename, ascending. Two entries in one
    // directory cannot share a filename, so this is a total order and the
    // result never depends on directory-entry order.
    files.sort_by(|(a_path, a_size), (b_path, b_size)| {
        b_size
            .cmp(a_size)
            .then_with(|| a_path.file_name().cmp(&b_path.file_name()))
    });
    files.into_iter().next().map(|(path, _)| path)
}

#[cfg(windows)]
fn registry_roots() -> Vec<PathBuf> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::w;

    let mut out = Vec::new();
    let queries = [
        (
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
        ),
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
        assert!(is_infrastructure(
            "228980",
            "Steamworks Common Redistributables"
        ));
        assert!(is_infrastructure("1070560", "Steam Linux Runtime"));
        assert!(!is_infrastructure("105600", "Terraria"));
    }

    #[test]
    fn filters_infrastructure_by_name() {
        assert!(is_infrastructure("999999", "Proton 9.0"));
        assert!(is_infrastructure(
            "999998",
            "Steam Linux Runtime 3.0 (sniper)"
        ));
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

    /// Gives each test its own scratch directory under the OS temp dir. The
    /// caller-supplied label is combined with this process's id and an
    /// atomically incremented counter so the name is unique both across
    /// concurrent `cargo test` processes (e.g. two collaborators or CI jobs
    /// sharing a runner) and across parallel test threads within one
    /// process; a fixed name would let them collide.
    fn scratch_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wso_steam_libcache_{label}_{pid}_{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn library_image_prefers_logo_png() {
        let dir = scratch_dir("prefers_logo");
        std::fs::write(dir.join("library_600x900.jpg"), vec![0u8; 500]).unwrap();
        std::fs::write(dir.join("logo.png"), vec![0u8; 10]).unwrap();

        assert_eq!(pick_library_image(&dir), Some(dir.join("logo.png")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn library_image_falls_back_to_600x900_when_logo_missing() {
        let dir = scratch_dir("falls_back_600x900");
        std::fs::write(
            dir.join("c3dff088e090f81d6e3d88eabbb67732647c69cf"),
            vec![0u8; 999_999],
        )
        .unwrap();
        std::fs::write(dir.join("library_600x900.jpg"), vec![0u8; 10]).unwrap();

        assert_eq!(
            pick_library_image(&dir),
            Some(dir.join("library_600x900.jpg"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn library_image_picks_the_largest_extensionless_file_as_last_resort() {
        let dir = scratch_dir("largest_extensionless");
        std::fs::write(dir.join("a1b2c3d4e5f6"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("f6e5d4c3b2a1"), vec![0u8; 500]).unwrap();

        assert_eq!(pick_library_image(&dir), Some(dir.join("f6e5d4c3b2a1")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two assets of exactly the same size is not exotic — Steam's cache is
    /// full of small, similarly-shaped files — and "largest remaining" alone
    /// does not decide between them. Without a tie-break the winner is
    /// whatever `read_dir` returned last, which no filesystem guarantees, so
    /// the same game could show one icon today and another tomorrow with
    /// nothing having changed.
    ///
    /// The alphabetically-first name is written to disk *second*, so a
    /// last-one-wins accident cannot produce the asserted answer. A third
    /// file that is genuinely larger is absent on purpose: this test must
    /// isolate the tie-break, not the size comparison.
    #[test]
    fn library_image_breaks_a_size_tie_by_filename_rather_than_directory_order() {
        let dir = scratch_dir("size_tie_broken_by_name");
        std::fs::write(dir.join("zzz_written_first"), vec![0u8; 500]).unwrap();
        std::fs::write(dir.join("aaa_written_second"), vec![0u8; 500]).unwrap();

        assert_eq!(
            pick_library_image(&dir),
            Some(dir.join("aaa_written_second"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The guard on the tie-break: it must only ever settle *ties*. A larger
    /// file whose name sorts last must still win outright.
    #[test]
    fn library_image_still_prefers_a_larger_file_over_an_earlier_name() {
        let dir = scratch_dir("size_beats_name");
        std::fs::write(dir.join("aaa_small"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.join("zzz_large"), vec![0u8; 500]).unwrap();

        assert_eq!(pick_library_image(&dir), Some(dir.join("zzz_large")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn library_image_is_none_for_an_empty_directory() {
        let dir = scratch_dir("empty");
        assert_eq!(pick_library_image(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn library_image_is_none_for_a_missing_directory() {
        // A directory that is guaranteed not to exist: built the same way as
        // `scratch_dir` but deliberately never created.
        let dir = scratch_dir("does_not_exist");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(pick_library_image(&dir), None);
    }
}
