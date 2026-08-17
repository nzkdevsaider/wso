//! One rule for "does this file have the extension I am looking for", used
//! by every directory scan in the crate.
//!
//! There used to be four separate answers to that question, and they did not
//! agree. `ops::scan_owned` and `icon::exe_finder::walk` compared
//! case-insensitively; `shortcut::url_legacy::find` and
//! `platform::epic::games` compared with `== Some("url")` and
//! `== Some("item")`. Windows filenames are case-insensitive, so the two
//! strict sites were simply wrong: a `Terraria.URL` was invisible to
//! `migrate`, and a `Fortnite.ITEM` manifest was invisible to the Epic
//! provider — the game would never be detected at all, with no error and
//! nothing to explain the absence.
//!
//! There is no site in this crate where a case-sensitive match is the
//! correct behavior, so this is one shared helper rather than a pair of
//! them.

use std::path::Path;

/// Whether `path` ends in `.{extension}`, compared the way Windows compares
/// filenames: ASCII-case-insensitively, and without the leading dot.
///
/// A path with no extension, or one whose extension is not valid UTF-8,
/// answers `false` — neither can match a plain ASCII extension.
pub fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn matches_the_extension_exactly_as_written() {
        assert!(has_extension(&PathBuf::from(r"C:\D\Terraria.lnk"), "lnk"));
        assert!(has_extension(&PathBuf::from("manifest.item"), "item"));
    }

    /// The whole point: File Explorer will happily produce `Terraria.URL`
    /// via a rename, and Windows treats it as exactly the same file type.
    /// A scan that missed it would silently skip a real file.
    #[test]
    fn matches_regardless_of_letter_case_on_either_side() {
        assert!(has_extension(&PathBuf::from(r"C:\D\Terraria.URL"), "url"));
        assert!(has_extension(&PathBuf::from(r"C:\D\Terraria.Url"), "URL"));
        assert!(has_extension(&PathBuf::from("Game.EXE"), "exe"));
    }

    /// Still a comparison: a different extension must not match, or every
    /// scan in the crate would claim every file it walked past.
    #[test]
    fn does_not_match_a_different_extension() {
        assert!(!has_extension(&PathBuf::from(r"C:\D\notes.txt"), "lnk"));
        assert!(!has_extension(&PathBuf::from(r"C:\D\Terraria.lnk"), "url"));
    }

    /// Only the final extension counts, and only as a whole: `.lnk` must not
    /// be found inside `Game.lnk.txt`, and a name that merely contains the
    /// letters must not match either.
    #[test]
    fn matches_only_the_final_extension_and_only_whole() {
        assert!(!has_extension(&PathBuf::from(r"C:\D\Game.lnk.txt"), "lnk"));
        assert!(has_extension(&PathBuf::from(r"C:\D\Game.lnk.txt"), "txt"));
        assert!(!has_extension(&PathBuf::from(r"C:\D\Game.linkage"), "lnk"));
    }

    /// Steam's library cache names assets after a content hash with no
    /// extension at all; those must simply not match, not panic.
    #[test]
    fn a_file_with_no_extension_matches_nothing() {
        assert!(!has_extension(
            &PathBuf::from("c3dff088e090f81d6e3d88eabbb67732647c69cf"),
            "lnk"
        ));
        assert!(!has_extension(&PathBuf::from(r"C:\D"), "lnk"));
    }
}
