//! Parses the legacy `.url` shortcuts Steam creates directly (a web
//! bookmark pointing at a `steam://` URI), so `migrate` can replace them
//! with real `.lnk` shortcuts.
//!
//! Correction 1 (see the task brief): a `.url`'s `IconFile=` line records an
//! icon Steam *intended* to resolve, not one it necessarily did. On the
//! maintainer's real Desktop, all eight Steam `.url` files carry an
//! `IconFile=` pointing at a hash-named `.ico` under
//! `Steam\steam\games\`, and every one of those eight files is missing —
//! that is the actual mechanism behind the blank-icon complaint this whole
//! tool exists to fix. So this module only *parses* the line; it is
//! `migrate` (not this module) that decides whether the path it names is
//! trustworthy, by checking it still exists on disk.

use std::path::{Path, PathBuf};

/// A shortcut Steam created, which is a web bookmark rather than a real
/// shortcut. These are what lose their icons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyUrl {
    pub path: PathBuf,
    pub appid: String,
    /// The icon Steam recorded for this shortcut, if any. This is a hint,
    /// not a resolved source — see the module doc comment. Callers must
    /// verify the path still exists before trusting it.
    pub icon_file: Option<PathBuf>,
}

/// Steam appids fit comfortably in 32 bits (at most 10 decimal digits).
/// `rungameid` values longer than this are composite ids for non-Steam
/// shortcuts and mods (Named Risk 2), which cannot be launched with
/// `-applaunch` — so they are rejected here rather than ever reaching
/// `migrate` and producing a shortcut that cannot actually launch anything.
const MAX_APPID_DIGITS: usize = 10;

/// Parses one `.url` file's contents. Returns `None` for anything that is
/// not a Steam `rungameid`/`launch` shortcut with a plausible appid —
/// including a plain web bookmark (Named Risk 3: not this tool's to touch)
/// and a composite id too long to be a real appid (Named Risk 2).
///
/// Tolerant of the extra sections and keys (`[{000214A0-...}]`, `Prop3=`,
/// `IDList=`, `IconIndex=`, ...) a real Steam `.url` carries alongside
/// `[InternetShortcut]`: every line is scanned independently for the two
/// prefixes this cares about, so anything else is simply not matched and
/// falls through unread.
pub fn parse(path: &Path, contents: &str) -> Option<LegacyUrl> {
    let mut appid = None;
    let mut icon_file = None;

    for line in contents.lines() {
        let line = line.trim();
        if let Some(url) = line.strip_prefix("URL=") {
            let tail = url
                .strip_prefix("steam://rungameid/")
                .or_else(|| url.strip_prefix("steam://launch/"))?;
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || digits.len() > MAX_APPID_DIGITS {
                return None;
            }
            appid = Some(digits);
        } else if let Some(icon) = line.strip_prefix("IconFile=") {
            if !icon.is_empty() {
                icon_file = Some(PathBuf::from(icon));
            }
        }
    }

    Some(LegacyUrl {
        path: path.to_path_buf(),
        appid: appid?,
        icon_file,
    })
}

/// Scans `dirs` for `.url` files that parse as Steam shortcuts. A directory
/// that cannot be read, a file that cannot be read, or a `.url` file that
/// does not parse as a Steam shortcut (a plain bookmark, Named Risk 3) are
/// all skipped silently rather than aborting the whole scan — the same
/// "one bad entry must not hide the rest" discipline `ops::scan_owned`
/// follows for `.lnk` files.
pub fn find(dirs: &[PathBuf]) -> Vec<LegacyUrl> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("url") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(parsed) = parse(&path, &text) {
                out.push(parsed);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEAM_URL: &str = "[InternetShortcut]\r\n\
URL=steam://rungameid/105600\r\n\
IconIndex=0\r\n\
IconFile=C:\\Program Files (x86)\\Steam\\steam\\games\\abc123.ico\r\n";

    /// The shape Steam actually writes on this maintainer's real machine: an
    /// extra `[{000214A0-...}]`/`Prop3=`/`IDList=` preamble ahead of
    /// `[InternetShortcut]`. Correction 1 found all eight real `.url` files
    /// on the Desktop carry this preamble, so a parser that only tolerates
    /// the brief's minimal fixture would still break on every real file.
    const REAL_STEAM_URL: &str = "[{000214A0-0000-0000-C000-000000000046}]\r\n\
Prop3=19,0\r\n\
[InternetShortcut]\r\n\
IDList=\r\n\
IconIndex=0\r\n\
URL=steam://rungameid/4000\r\n\
IconFile=C:\\Program Files (x86)\\Steam\\steam\\games\\01b545c997d68f6c4ac18ccef6c57d1d5d690e03.ico\r\n";

    #[test]
    fn parses_appid_and_icon() {
        let p = PathBuf::from(r"C:\Desktop\Terraria.url");
        let parsed = parse(&p, STEAM_URL).unwrap();
        assert_eq!(parsed.appid, "105600");
        assert_eq!(
            parsed.icon_file,
            Some(PathBuf::from(
                r"C:\Program Files (x86)\Steam\steam\games\abc123.ico"
            ))
        );
    }

    /// Proves the extra sections and keys a real Steam `.url` carries (see
    /// `REAL_STEAM_URL`) do not confuse the parser — only `URL=` and
    /// `IconFile=` are ever consulted, regardless of what else is present.
    #[test]
    fn tolerates_the_extra_sections_a_real_steam_url_carries() {
        let p = PathBuf::from(r"C:\Desktop\Garry's Mod.url");
        let parsed = parse(&p, REAL_STEAM_URL).unwrap();
        assert_eq!(parsed.appid, "4000");
        assert_eq!(
            parsed.icon_file,
            Some(PathBuf::from(
                r"C:\Program Files (x86)\Steam\steam\games\01b545c997d68f6c4ac18ccef6c57d1d5d690e03.ico"
            ))
        );
    }

    #[test]
    fn parses_without_an_icon_line() {
        let text = "[InternetShortcut]\r\nURL=steam://rungameid/440\r\n";
        let parsed = parse(Path::new("x.url"), text).unwrap();
        assert_eq!(parsed.appid, "440");
        assert_eq!(parsed.icon_file, None);
    }

    #[test]
    fn accepts_the_applaunch_form() {
        let text = "[InternetShortcut]\r\nURL=steam://launch/620\r\n";
        assert_eq!(parse(Path::new("x.url"), text).unwrap().appid, "620");
    }

    #[test]
    fn ignores_non_steam_urls() {
        let text = "[InternetShortcut]\r\nURL=https://example.com\r\n";
        assert!(parse(Path::new("x.url"), text).is_none());
    }

    #[test]
    fn ignores_a_rungameid_with_a_mod_suffix() {
        // Non-Steam shortcuts use 64-bit composite ids; those are not appids.
        let text = "[InternetShortcut]\r\nURL=steam://rungameid/14829735431807303680\r\n";
        assert!(parse(Path::new("x.url"), text).is_none());
    }

    /// `find` must only pick up files whose extension is `.url` — a stray
    /// `.lnk` or unrelated file sitting in the same directory must never be
    /// handed to `parse` at all.
    #[test]
    fn find_only_reads_dot_url_files() {
        let dir = super::tests_support::unique_dir("find_only_url");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Terraria.url"), STEAM_URL).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a url file").unwrap();

        let found = find(std::slice::from_ref(&dir));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].appid, "105600");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `.url` file that is a plain web bookmark (no `steam://` URL at all)
    /// must be invisible to `find` — Named Risk 3: it is not this tool's to
    /// touch.
    #[test]
    fn find_skips_a_non_steam_url_file() {
        let dir = super::tests_support::unique_dir("find_skips_bookmark");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Anthropic.url"),
            "[InternetShortcut]\r\nURL=https://www.anthropic.com\r\n",
        )
        .unwrap();

        let found = find(std::slice::from_ref(&dir));
        assert!(
            found.is_empty(),
            "a web bookmark must never be treated as a Steam shortcut"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A directory that does not exist must simply contribute nothing,
    /// rather than aborting the scan of the other directories passed in.
    #[test]
    fn find_skips_an_unreadable_directory_without_aborting() {
        let missing = super::tests_support::unique_dir("find_missing_dir");
        let dir = super::tests_support::unique_dir("find_missing_dir_ok");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Terraria.url"), STEAM_URL).unwrap();

        let found = find(&[missing, dir.clone()]);
        assert_eq!(
            found.len(),
            1,
            "the readable directory's file must still be found"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Test-only helper module so the unique-scratch-directory pattern used
/// throughout this crate's tests (process id + atomic counter, per
/// `tests/common::unique_temp_dir`) is available here too without
/// hardcoding a directory name.
#[cfg(test)]
mod tests_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    pub fn unique_dir(label: &str) -> PathBuf {
        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("wso_url_legacy_{label}_{pid}_{n}"))
    }
}
