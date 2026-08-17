use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum IconError {
    Decode(String),
    Write(String),
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconError::Decode(d) => write!(f, "could not decode image: {d}"),
            IconError::Write(d) => write!(f, "could not write icon: {d}"),
        }
    }
}

impl std::error::Error for IconError {}

/// Sizes Windows asks for across the Start Menu, the desktop and Alt-Tab.
const SIZES: [u32; 4] = [256, 48, 32, 16];

/// Converts a raster image into a multi-resolution `.ico`. Used only as the
/// fallback when a game's executable carries no usable icon.
///
/// Format is detected from the file's bytes, not its extension: Steam's
/// `librarycache` names many images after a content hash with no extension
/// at all, and some extensioned files carry a different format than their
/// name implies. `image::open` would fail on both; reading the bytes and
/// calling `image::load_from_memory` sniffs the real format instead.
pub fn png_to_ico(src: &Path, dest: &Path) -> Result<(), IconError> {
    let bytes = std::fs::read(src).map_err(|e| IconError::Decode(e.to_string()))?;
    let img = image::load_from_memory(&bytes).map_err(|e| IconError::Decode(e.to_string()))?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        // `resize` preserves aspect ratio, so pad onto a square canvas to keep
        // non-square logos from being distorted.
        let scaled = img.resize(size, size, image::imageops::FilterType::Lanczos3);
        let mut canvas = image::RgbaImage::new(size, size);
        let sx = (size - scaled.width()) / 2;
        let sy = (size - scaled.height()) / 2;
        image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), sx as i64, sy as i64);

        let entry = ico::IconImage::from_rgba_data(size, size, canvas.into_raw());
        dir.add_entry(
            ico::IconDirEntry::encode(&entry).map_err(|e| IconError::Write(e.to_string()))?,
        );
    }

    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| IconError::Write(e.to_string()))?;
    }

    write_atomically(dest, |w| dir.write(w))
}

/// Writes the bytes produced by `write` into `dest` only if the write fully
/// succeeds. The bytes go to a temporary file in the destination's own
/// directory first and are renamed into place afterward — same-directory
/// rename is atomic on Windows — and on any failure the temp file is
/// removed and `dest` is left completely untouched.
///
/// This matters because `resolve()`'s cache check only tests whether `dest`
/// exists: writing straight into `dest` and failing partway through (disk
/// full, a permission error, an antivirus lock mid-write) would leave a
/// corrupt or empty file there that gets served as a "valid" cached icon on
/// every subsequent run, forever, with no retry.
fn write_atomically(
    dest: &Path,
    write: impl FnOnce(&mut dyn std::io::Write) -> std::io::Result<()>,
) -> Result<(), IconError> {
    let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
    let file_name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("icon");
    let tmp_name = format!(".{file_name}.{}.tmp", std::process::id());
    let tmp: PathBuf = match parent {
        Some(p) => p.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };

    let outcome: Result<(), IconError> = (|| {
        let mut file = std::fs::File::create(&tmp).map_err(|e| IconError::Write(e.to_string()))?;
        write(&mut file).map_err(|e| IconError::Write(e.to_string()))
    })();

    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        IconError::Write(e.to_string())
    })
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
        std::env::temp_dir().join(format!("wso_convert_{label}_{pid}_{n}"))
    }

    /// Writes real PNG-encoded bytes to `path` regardless of its extension
    /// (or lack of one). `Image::save` picks its encoder from the
    /// destination's extension, which is exactly the behavior these tests
    /// must NOT exercise, so the format is fixed explicitly here instead.
    fn write_test_png(path: &std::path::Path, size: u32) {
        let mut img = image::RgbaImage::new(size, size);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
        let mut bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn converts_a_png_into_a_multi_size_ico() {
        let dir = scratch_dir("png_to_ico");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("logo.png");
        let dest = dir.join("out.ico");
        write_test_png(&src, 300);

        png_to_ico(&src, &dest).unwrap();

        assert!(dest.is_file());
        assert!(std::fs::metadata(&dest).unwrap().len() > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reports_a_missing_source() {
        let dir = scratch_dir("missing_source");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("wso_convert_missing.ico");
        assert!(matches!(
            png_to_ico(dir.join("nope.png").as_path(), &dest),
            Err(IconError::Decode(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The provider hands the converter Steam library-cache files named after
    /// a content hash with no extension at all. `image::open` would refuse
    /// these because it guesses format from the file name; the converter
    /// must sniff the format from the bytes instead, so a `.jpg`-free source
    /// still decodes.
    #[test]
    fn converts_a_source_with_no_extension() {
        let dir = scratch_dir("no_extension");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("c3dff088e090f81d6e3d88eabbb67732647c69cf");
        let dest = dir.join("out.ico");
        write_test_png(&src, 64);

        png_to_ico(&src, &dest).unwrap();

        assert!(dest.is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Proves format detection reads the bytes, not the name: this file
    /// holds PNG data but is named `.jpg`. If the converter trusted the
    /// extension, decoding would fail here even though the content is
    /// perfectly valid.
    #[test]
    fn converts_a_source_with_a_misleading_extension() {
        let dir = scratch_dir("misleading_extension");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("art.jpg");
        let dest = dir.join("out.ico");
        write_test_png(&src, 64);

        png_to_ico(&src, &dest).unwrap();

        assert!(dest.is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A conversion that can never even begin writing — because the
    /// destination's parent cannot be created — must leave nothing behind
    /// at the destination, not a stray empty file.
    #[test]
    fn a_failed_conversion_leaves_no_file_at_the_destination() {
        let dir = scratch_dir("failed_conversion_no_partial_file");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("logo.png");
        write_test_png(&src, 64);

        // `blocker` is a regular file, not a directory, so nothing can ever
        // be created underneath it — this deterministically forces
        // `png_to_ico` down its failure path without needing a real
        // disk-full or permission-denied condition.
        let blocker = dir.join("blocker.txt");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let dest = blocker.join("out.ico");

        assert!(png_to_ico(&src, &dest).is_err());
        assert!(
            !dest.exists(),
            "a failed conversion must not leave anything at the destination"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The exact bug the atomic write closes: a write that fails partway —
    /// after some bytes have already reached disk — must not leave those
    /// partial bytes sitting at `dest`. `resolve()`'s cache check only
    /// tests whether `dest` exists, so a half-written file left there would
    /// be served as a permanently "valid" cached icon on every later run,
    /// with no retry. Exercises `write_atomically` directly with a writer
    /// that succeeds partially and then fails, which a real disk-full or
    /// permission error cannot be forced to do deterministically.
    #[test]
    fn a_write_that_fails_partway_leaves_no_file_at_the_destination() {
        let dir = scratch_dir("partial_write_no_partial_file");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("440.ico");

        let result = write_atomically(&dest, |w| {
            w.write_all(b"only some of the bytes")?;
            Err(std::io::Error::other("simulated mid-write failure"))
        });

        assert!(result.is_err());
        assert!(
            !dest.exists(),
            "a failed write must not leave a partial file at the destination"
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "the temporary file must be cleaned up, not left behind"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
