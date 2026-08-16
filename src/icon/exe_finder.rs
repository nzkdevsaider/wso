use std::path::{Path, PathBuf};

/// Filename fragments that never identify a game. Matched case-insensitively
/// against the file stem.
const NAME_BLOCKLIST: [&str; 10] = [
    "crashhandler",
    "crashreport",
    "crashmailer",
    "setup",
    "installer",
    "uninst",
    "dotnetfx",
    "vcredist",
    "dxsetup",
    "sendrpt",
];

/// Directory names that only ever hold redistributables.
const DIR_BLOCKLIST: [&str; 5] = [
    "_commonredist",
    "redist",
    "directx",
    "dotnet",
    "vcredist",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeCandidate {
    pub rel_path: PathBuf,
    pub size: u64,
}

impl ExeCandidate {
    /// Number of path components, counting the filename. A file directly in
    /// the install directory has depth 1.
    pub fn depth(&self) -> usize {
        self.rel_path.components().count()
    }

    fn stem_lower(&self) -> String {
        self.rel_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    }

    fn is_blocklisted(&self) -> bool {
        let stem = self.stem_lower();
        if NAME_BLOCKLIST.iter().any(|b| stem.contains(b)) {
            return true;
        }
        self.rel_path
            .parent()
            .map(|p| {
                p.components().any(|c| {
                    let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
                    DIR_BLOCKLIST.contains(&s.as_str())
                })
            })
            .unwrap_or(false)
    }
}

/// Reduces a title to comparable letters and digits, so "Garry's Mod",
/// "GarrysMod" and "garrys mod" all normalize to the same thing.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Higher is better. 0 means no relationship at all.
fn name_score(candidate_stem: &str, game_name: &str, install_dir_name: &str) -> u32 {
    let stem = normalize(candidate_stem);
    if stem.is_empty() {
        return 0;
    }
    let mut best = 0;
    for target in [normalize(game_name), normalize(install_dir_name)] {
        if target.is_empty() {
            continue;
        }
        let score = if stem == target {
            100
        } else if stem.starts_with(&target) || target.starts_with(&stem) {
            60
        } else if stem.contains(&target) || target.contains(&stem) {
            40
        } else {
            0
        };
        best = best.max(score);
    }
    best
}

/// Picks the executable whose icon best represents the game.
///
/// Depth dominates: a small launcher in the install root beats a large tool
/// buried in `bin\win64\`. Within the shallowest populated depth, name
/// similarity decides, then file size.
pub fn pick_executable<'a>(
    game_name: &str,
    install_dir_name: &str,
    candidates: &'a [ExeCandidate],
) -> Option<&'a ExeCandidate> {
    let viable: Vec<&ExeCandidate> =
        candidates.iter().filter(|c| !c.is_blocklisted()).collect();

    let min_depth = viable.iter().map(|c| c.depth()).min()?;

    viable
        .into_iter()
        .filter(|c| c.depth() == min_depth)
        .max_by_key(|c| (name_score(&c.stem_lower(), game_name, install_dir_name), c.size))
}

/// Walks an install directory collecting `.exe` files, bounded by depth so a
/// game with a deep asset tree does not cost a full recursive scan.
pub fn collect_candidates(install_dir: &Path, max_depth: usize) -> Vec<ExeCandidate> {
    let mut out = Vec::new();
    walk(install_dir, install_dir, max_depth, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, remaining: usize, out: &mut Vec<ExeCandidate>) {
    if remaining == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk(root, &path, remaining - 1, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(ExeCandidate { rel_path: rel.to_path_buf(), size: meta.len() });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(path: &str, size: u64) -> ExeCandidate {
        ExeCandidate { rel_path: PathBuf::from(path), size }
    }

    fn pick<'a>(name: &str, dir: &str, cands: &'a [ExeCandidate]) -> &'a str {
        pick_executable(name, dir, cands)
            .expect("expected a candidate")
            .rel_path
            .to_str()
            .unwrap()
    }

    #[test]
    fn terraria_prefers_the_game_over_the_server() {
        let cands = [
            c("Terraria.exe", 25_990_656),
            c("TerrariaServer.exe", 25_420_288),
            c("dotNetFx40_Full_x86_x64.exe", 1_600_220),
            c("dotNetFx40_Full_setup.exe", 889_416),
        ];
        assert_eq!(pick("Terraria", "Terraria", &cands), "Terraria.exe");
    }

    #[test]
    fn persona_3_reload_descends_into_unreal_layout() {
        let cands = [
            c(r"P3R\Binaries\Win64\P3R.exe", 366_251_968),
            c(r"Engine\Binaries\Win64\CrashReportClient.exe", 19_487_232),
        ];
        assert_eq!(
            pick("Persona 3 Reload", "P3R", &cands),
            r"P3R\Binaries\Win64\P3R.exe"
        );
    }

    #[test]
    fn monster_hunter_wilds_ignores_a_huge_crash_reporter() {
        let cands = [
            c("MonsterHunterWilds.exe", 539_469_728),
            c("CrashReport.exe", 155_125_104),
            c("InstallerMessage.exe", 1_623_120),
        ];
        assert_eq!(
            pick("Monster Hunter Wilds", "MonsterHunterWilds", &cands),
            "MonsterHunterWilds.exe"
        );
    }

    #[test]
    fn overwatch_ignores_nested_helpers() {
        let cands = [
            c("Overwatch.exe", 65_078_992),
            c(r"BlizzardBrowser\BlizzardBrowser.exe", 2_267_856),
            c(r"ErrorReporting\x86\BlizzardError.exe", 908_416),
            c(r"ErrorReporting\x64\CrashMailer_64.exe", 450_768),
        ];
        assert_eq!(pick("Overwatch®", "Overwatch", &cands), "Overwatch.exe");
    }

    #[test]
    fn x4_matches_on_a_partial_name() {
        let cands = [
            c("X4.exe", 55_412_768),
            c(r"support\drdump\sendrpt.exe", 911_872),
        ];
        assert_eq!(pick("X4: Foundations", "X4 Foundations", &cands), "X4.exe");
    }

    /// Regression: the largest executable is a Source modelling tool three
    /// levels deep. Depth must outrank size.
    #[test]
    fn garrys_mod_prefers_the_shallow_launcher_over_a_bigger_tool() {
        let cands = [
            c("gmod.exe", 430_928),
            c(r"bin\win64\studiomdl.exe", 2_818_384),
            c(r"bin\studiomdl.exe", 2_256_208),
            c(r"bin\hlfaceposer.exe", 2_121_552),
            c(r"bin\win64\vbsp.exe", 1_437_520),
            c(r"bin\win64\gmod.exe", 1_042_256),
        ];
        assert_eq!(pick("Garry's Mod", "GarrysMod", &cands), "gmod.exe");
    }

    #[test]
    fn helldivers_2_beats_a_similarly_sized_installer() {
        let cands = [
            c(r"bin\helldivers2.exe", 14_892_136),
            c(r"tools\GGSetup.exe", 13_163_048),
            c(r"bin\crs-handler.exe", 1_188_520),
            c(r"tools\gguninst.exe", 79_928),
        ];
        assert_eq!(
            pick("HELLDIVERS™ 2", "Helldivers 2", &cands),
            r"bin\helldivers2.exe"
        );
    }

    /// Regression: Unity's crash handler is physically larger than the game.
    #[test]
    fn supermarket_together_ignores_the_unity_crash_handler() {
        let cands = [
            c("UnityCrashHandler64.exe", 991_992),
            c("Supermarket Together.exe", 667_648),
        ];
        assert_eq!(
            pick("Supermarket Together", "Supermarket Together", &cands),
            "Supermarket Together.exe"
        );
    }

    #[test]
    fn returns_none_when_everything_is_blocklisted() {
        let cands = [c("vcredist_x64.exe", 100), c("UnityCrashHandler64.exe", 200)];
        assert!(pick_executable("Some Game", "SomeGame", &cands).is_none());
    }

    #[test]
    fn returns_none_for_an_empty_directory() {
        assert!(pick_executable("Some Game", "SomeGame", &[]).is_none());
    }

    #[test]
    fn depth_counts_the_filename() {
        assert_eq!(c("a.exe", 1).depth(), 1);
        assert_eq!(c(r"bin\a.exe", 1).depth(), 2);
        assert_eq!(c(r"a\b\c\d.exe", 1).depth(), 4);
    }
}
