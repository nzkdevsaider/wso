use std::path::{Path, PathBuf};

/// Filename fragments that never identify a game. Matched case-insensitively
/// against the file stem.
///
/// `busybox` and `quickeditdisable` were added after tModLoader was found to
/// resolve to `LaunchUtils\busybox64.exe` on a real install: the game ships
/// no `.exe` at all that represents it (it launches from a `.bat`), only
/// these two helper utilities, and `busybox64.exe` was confirmed to carry an
/// icon resource of its own — so without this entry `has_icon` would accept
/// it and the shortcut would show BusyBox's generic icon instead of falling
/// through to the game's librarycache artwork.
const NAME_BLOCKLIST: [&str; 12] = [
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
    "busybox",
    "quickeditdisable",
];

/// Directory names that only ever hold redistributables.
const DIR_BLOCKLIST: [&str; 5] = ["_commonredist", "redist", "directx", "dotnet", "vcredist"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeCandidate {
    pub rel_path: PathBuf,
    pub size: u64,
}

impl ExeCandidate {
    /// Number of path components, counting the filename. A file directly in
    /// the install directory has depth 1.
    ///
    /// Windows-specific, and load-bearing: this counts components the way
    /// `Path::components` splits them, which treats `\` as a separator only
    /// on Windows. In production the paths come from `walk` below, which
    /// builds them from real directory entries, so the platform's own
    /// separator is always what is there. In tests the paths are literals
    /// like `r"bin\tool.exe"` — on any other platform those are a single
    /// component, every candidate reports depth 1, and the depth rule
    /// silently stops being exercised at all. That is why the tests holding
    /// such literals are `#[cfg(windows)]`-gated rather than left to pass
    /// vacuously.
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
    let viable: Vec<&ExeCandidate> = candidates.iter().filter(|c| !c.is_blocklisted()).collect();

    let min_depth = viable.iter().map(|c| c.depth()).min()?;

    viable
        .into_iter()
        .filter(|c| c.depth() == min_depth)
        .max_by_key(|c| {
            (
                name_score(&c.stem_lower(), game_name, install_dir_name),
                c.size,
            )
        })
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
        } else if crate::paths::has_extension(&path, "exe")
            && let Ok(rel) = path.strip_prefix(root) {
                out.push(ExeCandidate {
                    rel_path: rel.to_path_buf(),
                    size: meta.len(),
                });
            }
    }
}

// `#[cfg(windows)]` as well as `#[cfg(test)]`: every candidate path below
// is a literal like `r"bin\tool.exe"`, and `\` is a path separator only
// on Windows. Anywhere else `ExeCandidate::depth` counts all of them as a
// single component, every candidate reports depth 1, and the depth rule
// stops being exercised — the regression corpus in particular would keep
// passing while proving nothing about the rule it was captured for.
// Refusing to compile off Windows is honest; passing vacuously is not.
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn c(path: &str, size: u64) -> ExeCandidate {
        ExeCandidate {
            rel_path: PathBuf::from(path),
            size,
        }
    }

    fn pick<'a>(name: &str, dir: &str, cands: &'a [ExeCandidate]) -> &'a str {
        pick_executable(name, dir, cands)
            .expect("expected a candidate")
            .rel_path
            .to_str()
            .unwrap()
    }

    #[test]
    fn depth_counts_the_filename() {
        assert_eq!(c("a.exe", 1).depth(), 1);
        assert_eq!(c(r"bin\a.exe", 1).depth(), 2);
        assert_eq!(c(r"a\b\c\d.exe", 1).depth(), 4);
    }

    // Group A: rule tests.
    //
    // Each test here is synthetic and named for the single rule in
    // `pick_executable` it proves, not for any game. Every candidate list is
    // built so that only the named rule can produce the asserted winner:
    // every other criterion either ties or points the other way. See each
    // test's comment for how it was verified to fail when its rule is
    // removed.
    mod rule_tests {
        use super::*;

        /// Isolates the exact-match tier of `name_score` (100) from the
        /// prefix tier (60). Both candidates sit at depth 1 and have equal
        /// size, so neither depth nor size can decide this; only the name
        /// score can. The exact match is placed *first* and the prefix
        /// match *last* on purpose: `Iterator::max_by_key` returns the last
        /// maximal element on a tie, so if the exact/prefix distinction
        /// were removed (collapsing both to the same score), the tie-break
        /// would hand it to the last element — the prefix match — instead
        /// of the exact one, and the test would fail.
        ///
        /// Verified: temporarily changed `name_score` so the `stem == target`
        /// branch fell through to the prefix branch (i.e. exact matches
        /// scored 60 instead of 100). Both candidates then tied at 60 and
        /// equal size, so the last element (`GameLauncher.exe`) won and the
        /// assertion failed as expected.
        #[test]
        fn exact_name_match_outranks_a_prefix_match() {
            let cands = [c("Game.exe", 100), c("GameLauncher.exe", 100)];
            assert_eq!(pick("Game", "Game", &cands), "Game.exe");
        }

        /// Isolates "some relation to the target" from "no relation at
        /// all". Both candidates sit at depth 1 with equal size, so this
        /// can only be decided by whether `name_score` treats a prefix
        /// match as better than an unrelated name. The prefix match is
        /// placed first and the unrelated name last, for the same
        /// tie-break reason as above.
        ///
        /// Verified: temporarily made `name_score` return 0 whenever
        /// `stem != target` (removing the prefix and substring tiers
        /// entirely). Both candidates then tied at 0 and equal size, so the
        /// last element (`Random.exe`) won and the assertion failed.
        #[test]
        fn prefix_match_outranks_no_relation() {
            let cands = [c("GameLauncher.exe", 100), c("Random.exe", 100)];
            assert_eq!(pick("Game", "Game", &cands), "GameLauncher.exe");
        }

        /// Isolates depth filtering from size. Both stems are unrelated to
        /// the game/install names (`name_score` ties at 0 for both), so
        /// without the depth cut the much larger deep file would win on
        /// size alone.
        ///
        /// Verified: temporarily replaced the `filter(|c| c.depth() ==
        /// min_depth)` step with a no-op so every viable candidate stayed in
        /// the running. `bin\tool.exe` then won on size (999_999 > 100) and
        /// the assertion failed.
        #[test]
        fn shallower_depth_outranks_a_larger_file_at_greater_depth() {
            let cands = [c("launcher.exe", 100), c(r"bin\tool.exe", 999_999)];
            assert_eq!(pick("Game", "Game", &cands), "launcher.exe");
        }

        /// Isolates depth filtering from name score. The deep candidate is
        /// an *exact* name match (the best possible score) while the
        /// shallow one has no name relation at all, yet the shallow one
        /// must still win: depth is applied before name score is ever
        /// consulted, so a perfect name match at greater depth never even
        /// enters the comparison.
        ///
        /// Verified: with the same no-op depth filter described above, the
        /// exact match `bin\Game.exe` (score 100) beat `Launcher.exe`
        /// (score 0) and the assertion failed.
        #[test]
        fn shallower_depth_outranks_an_equal_name_match_at_greater_depth() {
            let cands = [c("Launcher.exe", 100), c(r"bin\Game.exe", 100)];
            assert_eq!(pick("Game", "Game", &cands), "Launcher.exe");
        }

        /// Isolates the name blocklist. Both candidates sit at depth 1 and
        /// both have a name score of 0 (neither stem relates to the game or
        /// install name), so without the blocklist the larger file would
        /// win on size alone; the blocklist must remove it first.
        ///
        /// Verified: temporarily made `is_blocklisted` always return
        /// `false`. `Setup.exe` then won on size (999_999 > 100) and the
        /// assertion failed.
        #[test]
        fn name_blocklist_excludes_a_candidate_that_would_otherwise_win() {
            let cands = [c("Setup.exe", 999_999), c("Launcher.exe", 100)];
            assert_eq!(pick("Game", "Game", &cands), "Launcher.exe");
        }

        /// The ancestor-directory blocklist must exclude a decoy even when
        /// that decoy shares the real game's exact name (so it would
        /// otherwise tie on name score and win outright on size), and must
        /// do so case-insensitively: the path segment here is
        /// `_CommonRedist`, the constant is `_commonredist`.
        ///
        /// Verified: temporarily made `is_blocklisted` always return
        /// `false`. `_CommonRedist\Game.exe` then tied on name score and won
        /// on size (999_999 > 100), and the assertion failed.
        #[test]
        fn directory_blocklist_excludes_a_candidate_that_would_otherwise_win() {
            let cands = [
                c(r"_CommonRedist\Game.exe", 999_999),
                c(r"bin\Game.exe", 100),
            ];
            assert_eq!(pick("Game", "Game", &cands), r"bin\Game.exe");
        }

        /// Synthetic, unlike the regression corpus: no candidate pair in the
        /// real nine-game library this codebase was validated against ever
        /// actually reaches the size comparison — every real case is
        /// already decided by depth, by the blocklist, or by an unequal
        /// name score first. Both stems here are unrelated to the
        /// game/install names, so `name_score` ties at 0 for both and
        /// `size` is left as the only thing that can decide it.
        ///
        /// Verified: temporarily replaced the `c.size` term in the
        /// `max_by_key` tuple with a constant, so size could no longer
        /// affect the ordering. Both candidates then tied on the whole
        /// tuple, the last element (`bin\launcher.exe`) won, and the
        /// assertion failed.
        ///
        /// The larger candidate is listed first and the smaller one last on
        /// purpose: `Iterator::max_by_key` returns the *last* maximal
        /// element on a tie, so if size were not actually compared, "last
        /// item in the slice" would accidentally produce the same answer as
        /// "largest size" when the bigger one is placed last. Putting it
        /// first instead means only a real size comparison can make this
        /// pass.
        #[test]
        fn size_breaks_a_tie_when_name_scores_are_equal() {
            let cands = [c(r"tools\launcher.exe", 500), c(r"bin\launcher.exe", 100)];
            assert_eq!(pick("Some Game", "SomeGame", &cands), r"tools\launcher.exe");
        }

        #[test]
        fn returns_none_when_everything_is_blocklisted() {
            let cands = [
                c("vcredist_x64.exe", 100),
                c("UnityCrashHandler64.exe", 200),
            ];
            assert!(pick_executable("Some Game", "SomeGame", &cands).is_none());
        }

        #[test]
        fn returns_none_for_an_empty_directory() {
            assert!(pick_executable("Some Game", "SomeGame", &[]).is_none());
        }
    }

    // Group B: the regression corpus.
    //
    // These are frozen observations captured from one real nine-game Steam
    // installation in August 2026. They never touch the filesystem, they do
    // not depend on any of these games being installed on the machine
    // running the tests, and they are not updated when games are installed
    // or removed. Each entry pins a real executable-selection outcome as a
    // regression; `decided_by` records which rule in `pick_executable`
    // actually produces the result, so a reader can see what the entry
    // demonstrates without having to reverse-engineer it from the candidate
    // list. Several entries here are decided by a different mechanism than
    // their game's install layout might suggest at a glance — that mismatch
    // between what a name implies and what a test actually exercises is
    // exactly why this table records `decided_by` explicitly instead of
    // leaving it for the reader to infer.
    mod regression_corpus {
        use super::*;

        struct CorpusEntry {
            /// The game's display name, as reported by the storefront.
            game_name: &'static str,
            /// The install directory name on disk.
            install_dir_name: &'static str,
            /// (relative path, file size in bytes) for every `.exe` found
            /// under the install directory, bounded by the same depth used
            /// in production.
            candidates: &'static [(&'static str, u64)],
            /// The relative path `pick_executable` is expected to return, or
            /// `None` for a game with no viable candidate at all.
            expected: Option<&'static str>,
            /// Which rule in `pick_executable` actually decides this entry.
            decided_by: &'static str,
        }

        const CORPUS: &[CorpusEntry] = &[
            CorpusEntry {
                game_name: "Terraria",
                install_dir_name: "Terraria",
                candidates: &[
                    ("Terraria.exe", 25_990_656),
                    ("TerrariaServer.exe", 25_420_288),
                    ("dotNetFx40_Full_x86_x64.exe", 1_600_220),
                    ("dotNetFx40_Full_setup.exe", 889_416),
                ],
                expected: Some("Terraria.exe"),
                decided_by: "name score: both dotNetFx candidates are removed by the \
                    name blocklist (\"dotnetfx\"); between the two survivors, an exact \
                    match (100) beats TerrariaServer's prefix match (60). Terraria.exe \
                    also happens to be the larger file, but size is never consulted \
                    because the name scores already differ.",
            },
            CorpusEntry {
                game_name: "Persona 3 Reload",
                install_dir_name: "P3R",
                candidates: &[
                    (r"P3R\Binaries\Win64\P3R.exe", 366_251_968),
                    (r"Engine\Binaries\Win64\CrashReportClient.exe", 19_487_232),
                ],
                expected: Some(r"P3R\Binaries\Win64\P3R.exe"),
                decided_by: "name blocklist: both candidates sit at the same depth (4), \
                    so depth does not distinguish them. CrashReportClient.exe is removed \
                    by the name blocklist (\"crashreport\"), leaving P3R.exe as the only \
                    viable candidate.",
            },
            CorpusEntry {
                game_name: "Monster Hunter Wilds",
                install_dir_name: "MonsterHunterWilds",
                candidates: &[
                    ("MonsterHunterWilds.exe", 539_469_728),
                    ("CrashReport.exe", 155_125_104),
                    ("InstallerMessage.exe", 1_623_120),
                ],
                expected: Some("MonsterHunterWilds.exe"),
                decided_by: "name blocklist: CrashReport.exe (\"crashreport\") and \
                    InstallerMessage.exe (\"installer\") are both removed, leaving \
                    MonsterHunterWilds.exe as the only viable candidate.",
            },
            CorpusEntry {
                game_name: "Overwatch®",
                install_dir_name: "Overwatch",
                candidates: &[
                    ("Overwatch.exe", 65_078_992),
                    (r"BlizzardBrowser\BlizzardBrowser.exe", 2_267_856),
                    (r"ErrorReporting\x86\BlizzardError.exe", 908_416),
                    (r"ErrorReporting\x64\CrashMailer_64.exe", 450_768),
                ],
                expected: Some("Overwatch.exe"),
                decided_by: "depth: Overwatch.exe is the only depth-1 candidate. The \
                    blocklist also removes CrashMailer_64.exe (\"crashmailer\"), but that \
                    candidate was already at depth 3 and so never affected the \
                    minimum-depth calculation.",
            },
            CorpusEntry {
                game_name: "X4: Foundations",
                install_dir_name: "X4 Foundations",
                candidates: &[
                    ("X4.exe", 55_412_768),
                    (r"support\drdump\sendrpt.exe", 911_872),
                ],
                expected: Some("X4.exe"),
                decided_by: "depth alone: X4.exe is the only depth-1 candidate. The other \
                    file is also removed by the name blocklist (\"sendrpt\"), but it is \
                    three levels deep either way, so depth already settles it.",
            },
            CorpusEntry {
                game_name: "Garry's Mod",
                install_dir_name: "GarrysMod",
                candidates: &[
                    ("gmod.exe", 430_928),
                    (r"bin\win64\studiomdl.exe", 2_818_384),
                    (r"bin\studiomdl.exe", 2_256_208),
                    (r"bin\hlfaceposer.exe", 2_121_552),
                    (r"bin\win64\vbsp.exe", 1_437_520),
                    (r"bin\win64\gmod.exe", 1_042_256),
                ],
                expected: Some("gmod.exe"),
                decided_by: "depth alone: gmod.exe is the only depth-1 candidate. This is \
                    the shape that motivates the depth-before-size rule in the first \
                    place: a 430 KB launcher in the install root beats a 2.8 MB modelling \
                    tool three levels down in bin\\win64\\, which no synthetic test would \
                    have thought to invent.",
            },
            CorpusEntry {
                game_name: "HELLDIVERS™ 2",
                install_dir_name: "Helldivers 2",
                candidates: &[
                    (r"bin\helldivers2.exe", 14_892_136),
                    (r"tools\GGSetup.exe", 13_163_048),
                    (r"bin\crs-handler.exe", 1_188_520),
                    (r"tools\gguninst.exe", 79_928),
                ],
                expected: Some(r"bin\helldivers2.exe"),
                decided_by: "name blocklist plus name score, not size: GGSetup.exe \
                    (\"setup\") and gguninst.exe (\"uninst\") are removed first, even \
                    though GGSetup.exe is close in size to the winner. Between the two \
                    survivors at depth 2, helldivers2.exe is an exact name match (100) \
                    against crs-handler.exe's unrelated name (0).",
            },
            CorpusEntry {
                game_name: "Supermarket Together",
                install_dir_name: "Supermarket Together",
                candidates: &[
                    ("UnityCrashHandler64.exe", 991_992),
                    ("Supermarket Together.exe", 667_648),
                ],
                expected: Some("Supermarket Together.exe"),
                decided_by: "name blocklist: UnityCrashHandler64.exe (\"crashhandler\") is \
                    removed despite being the physically larger file, leaving \
                    Supermarket Together.exe as the only viable candidate.",
            },
            CorpusEntry {
                game_name: "tModLoader",
                install_dir_name: "tModLoader",
                candidates: &[
                    (r"LaunchUtils\busybox64.exe", 642_560),
                    (r"dotnet\dotnet.exe", 142_096),
                    (r"LaunchUtils\QuickEditDisable.exe", 5_120),
                ],
                expected: None,
                decided_by: "name blocklist and directory blocklist together remove every \
                    candidate: busybox64.exe and QuickEditDisable.exe are name-blocklisted \
                    (\"busybox\", \"quickeditdisable\"), and dotnet.exe sits under a \
                    directory-blocklisted `dotnet\\` folder. The game ships no executable \
                    that represents it at all — it launches from a `.bat` file, and \
                    busybox64.exe was independently confirmed to carry its own icon \
                    resource, so without the blocklist entries the resolver would have \
                    accepted it and shown BusyBox's generic icon instead of falling \
                    through to the game's librarycache artwork. `pick_executable` \
                    reporting `None` here is the truthful answer for a game with no \
                    representative executable.",
            },
        ];

        #[test]
        fn regression_corpus_matches_frozen_observations() {
            for entry in CORPUS {
                let candidates: Vec<ExeCandidate> = entry
                    .candidates
                    .iter()
                    .map(|(path, size)| c(path, *size))
                    .collect();
                let picked = pick_executable(entry.game_name, entry.install_dir_name, &candidates);
                match entry.expected {
                    Some(expected) => {
                        let picked = picked.unwrap_or_else(|| {
                            panic!(
                                "{}: expected {:?}, got None; decided by {}",
                                entry.game_name, expected, entry.decided_by
                            )
                        });
                        assert_eq!(
                            picked.rel_path.to_str().unwrap(),
                            expected,
                            "{}: decided by {}",
                            entry.game_name,
                            entry.decided_by
                        );
                    }
                    None => {
                        assert!(
                            picked.is_none(),
                            "{}: expected None, got {:?}; decided by {}",
                            entry.game_name,
                            picked.map(|p| p.rel_path.clone()),
                            entry.decided_by
                        );
                    }
                }
            }
        }
    }
}
