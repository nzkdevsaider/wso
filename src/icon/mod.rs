pub mod convert;
pub mod exe_finder;
pub mod probe;

use crate::platform::IconCandidate;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIcon {
    pub path: PathBuf,
    pub index: i32,
}

/// A `key` is safe to use as a cache filename component only if it cannot
/// influence which directory the generated file lands in. Bad keys are
/// rejected rather than sanitized: repairing two different bad keys (say,
/// by stripping separators) could collapse them onto the same cache
/// filename and show one game's icon for another, and a wrong icon is a
/// worse failure than falling back to a generic one.
fn is_safe_cache_key(key: &str) -> bool {
    !key.is_empty()
        && key != "."
        && !key.contains("..")
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Walks a game's ordered icon candidates and returns the first that yields
/// something Windows can actually draw.
///
/// `cache_dir` receives generated `.ico` files, named by `key` so repeated
/// runs reuse them instead of re-encoding. `key` must consist only of
/// `[A-Za-z0-9_.-]`, must not be `.` or `..`, and must not contain `..` as a
/// substring; a `key` that fails this check is treated as if its `ImageFile`
/// candidate did not exist, so the walk degrades to the next candidate
/// instead of writing outside `cache_dir`.
pub fn resolve(candidates: &[IconCandidate], cache_dir: &Path, key: &str) -> Option<ResolvedIcon> {
    for candidate in candidates {
        match candidate {
            IconCandidate::Executable(path) => {
                if probe::has_icon(path) {
                    return Some(ResolvedIcon {
                        path: path.clone(),
                        index: 0,
                    });
                }
            }
            IconCandidate::ImageFile(src) => {
                if !src.is_file() || !is_safe_cache_key(key) {
                    continue;
                }
                let dest = cache_dir.join(format!("{key}.ico"));
                if dest.is_file() {
                    return Some(ResolvedIcon {
                        path: dest,
                        index: 0,
                    });
                }
                if convert::png_to_ico(src, &dest).is_ok() {
                    return Some(ResolvedIcon {
                        path: dest,
                        index: 0,
                    });
                }
            }
            IconCandidate::Fallback(path) => {
                if path.is_file() {
                    return Some(ResolvedIcon {
                        path: path.clone(),
                        index: 0,
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        std::env::temp_dir().join(format!("wso_resolve_{label}_{pid}_{n}"))
    }

    /// A missing `Executable` candidate must be skipped in favor of a later
    /// candidate, not stop the walk. `has_icon` rejecting a nonexistent path
    /// (proven separately in `probe::tests`) is the only thing that can make
    /// this test land on the fallback instead of erroring or returning it by
    /// coincidence.
    #[test]
    fn skips_a_missing_executable_and_takes_the_fallback() {
        let dir = scratch_dir("missing_exe_falls_back");
        std::fs::create_dir_all(&dir).unwrap();
        let fallback = dir.join("fallback.exe");
        std::fs::write(&fallback, b"x").unwrap();

        let candidates = vec![
            IconCandidate::Executable(dir.join("missing.exe")),
            IconCandidate::Fallback(fallback.clone()),
        ];
        let resolved = resolve(&candidates, &dir, "123").unwrap();

        assert_eq!(resolved.path, fallback);
        assert_eq!(resolved.index, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An `ImageFile` candidate must be converted into the cache directory
    /// under `{key}.ico`, since that is the only candidate present after the
    /// missing executable — no other branch could produce this exact path.
    #[test]
    fn converts_an_image_candidate_into_the_cache() {
        let dir = scratch_dir("image_converts_to_cache");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("logo.png");
        image::RgbaImage::new(64, 64).save(&png).unwrap();

        let candidates = vec![
            IconCandidate::Executable(dir.join("missing.exe")),
            IconCandidate::ImageFile(png),
        ];
        let resolved = resolve(&candidates, &cache, "440").unwrap();

        assert_eq!(resolved.path, cache.join("440.ico"));
        assert!(resolved.path.is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A cached `.ico` from a previous run must be reused as-is rather than
    /// re-encoded. Proven by pre-seeding the cache with a file that is not
    /// valid icon data at all: if the resolver tried to convert the image
    /// candidate again, `png_to_ico` would overwrite this sentinel content,
    /// so an unchanged file is the only way this assertion can pass.
    #[test]
    fn reuses_an_existing_cached_icon_instead_of_re_encoding() {
        let dir = scratch_dir("reuses_cache");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let png = dir.join("logo.png");
        image::RgbaImage::new(64, 64).save(&png).unwrap();
        let cached = cache.join("440.ico");
        std::fs::write(&cached, b"not-a-real-ico-just-a-sentinel").unwrap();

        let candidates = vec![IconCandidate::ImageFile(png)];
        let resolved = resolve(&candidates, &cache, "440").unwrap();

        assert_eq!(resolved.path, cached);
        assert_eq!(
            std::fs::read(&resolved.path).unwrap(),
            b"not-a-real-ico-just-a-sentinel"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// When every candidate fails to validate, `resolve` must report that
    /// plainly with `None` rather than panicking or fabricating a path.
    #[test]
    fn returns_none_when_nothing_resolves() {
        let dir = scratch_dir("nothing_resolves");
        std::fs::create_dir_all(&dir).unwrap();
        let candidates = vec![IconCandidate::Executable(dir.join("missing.exe"))];
        assert!(resolve(&candidates, &dir, "1").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn safe_cache_key_accepts_plain_alphanumeric_ids() {
        assert!(is_safe_cache_key("440"));
        assert!(is_safe_cache_key("SomeCatalogId_1.2-3"));
    }

    #[test]
    fn safe_cache_key_rejects_a_forward_slash() {
        assert!(!is_safe_cache_key("a/b"));
    }

    #[test]
    fn safe_cache_key_rejects_a_backslash() {
        assert!(!is_safe_cache_key("a\\b"));
    }

    /// A colon lets a key smuggle in a drive letter (`C:evil`), which
    /// `Path::join` would treat as absolute and use to replace `cache_dir`
    /// outright rather than nest under it.
    #[test]
    fn safe_cache_key_rejects_a_colon() {
        assert!(!is_safe_cache_key("C:evil"));
    }

    #[test]
    fn safe_cache_key_rejects_a_lone_dot() {
        assert!(!is_safe_cache_key("."));
    }

    #[test]
    fn safe_cache_key_rejects_a_traversal_segment() {
        assert!(!is_safe_cache_key(".."));
        assert!(!is_safe_cache_key("a..b"));
    }

    /// The only candidate present is a real, valid `ImageFile` — so if
    /// `resolve` reaches the `Fallback` candidate at all, it can only be
    /// because the unsafe `key` made it skip the image candidate, not
    /// because the image was missing or failed to convert.
    #[test]
    fn rejects_a_traversal_key_and_falls_back_instead_of_writing_outside_the_cache() {
        let dir = scratch_dir("traversal_key_falls_back");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let png = dir.join("logo.png");
        image::RgbaImage::new(64, 64).save(&png).unwrap();
        let fallback = dir.join("fallback.exe");
        std::fs::write(&fallback, b"x").unwrap();

        let candidates = vec![
            IconCandidate::ImageFile(png),
            IconCandidate::Fallback(fallback.clone()),
        ];
        let resolved = resolve(&candidates, &cache, "..\\..\\evil").unwrap();

        assert_eq!(resolved.path, fallback);
        assert_eq!(
            std::fs::read_dir(&cache).unwrap().count(),
            0,
            "an unsafe key must not cause anything to be written, in the cache or elsewhere"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Same rule, forced through a single embedded separator rather than a
    /// full `..` traversal — a key doesn't need to look like a traversal
    /// attack to be able to escape `cache_dir` into an unintended
    /// subdirectory.
    #[test]
    fn rejects_a_key_with_a_path_separator_and_falls_back() {
        let dir = scratch_dir("separator_key_falls_back");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let png = dir.join("logo.png");
        image::RgbaImage::new(64, 64).save(&png).unwrap();
        let fallback = dir.join("fallback.exe");
        std::fs::write(&fallback, b"x").unwrap();

        let candidates = vec![
            IconCandidate::ImageFile(png),
            IconCandidate::Fallback(fallback.clone()),
        ];
        let resolved = resolve(&candidates, &cache, "sub/evil").unwrap();

        assert_eq!(resolved.path, fallback);
        assert!(!cache.join("sub").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
