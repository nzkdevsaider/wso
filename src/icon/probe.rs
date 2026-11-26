use std::path::Path;

/// True when Windows can draw an icon out of this file. An executable with no
/// icon resource must fall through to the next candidate rather than produce
/// a blank shortcut.
#[cfg(windows)]
pub fn has_icon(path: &Path) -> bool {
    use windows::Win32::UI::Shell::ExtractIconExW;
    use windows::core::HSTRING;

    if !path.is_file() {
        return false;
    }
    let wide = HSTRING::from(path.as_os_str());
    let count = unsafe { ExtractIconExW(&wide, -1, None, None, 0) };
    count > 0
}

#[cfg(not(windows))]
pub fn has_icon(path: &Path) -> bool {
    path.is_file()
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
        std::env::temp_dir().join(format!("wso_probe_{label}_{pid}_{n}"))
    }

    #[test]
    fn missing_files_have_no_icon() {
        assert!(!has_icon(Path::new(r"C:\definitely\not\here.exe")));
    }

    #[cfg(windows)]
    #[test]
    fn a_system_executable_has_an_icon() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(has_icon(Path::new(&windir).join("explorer.exe").as_path()));
    }

    #[cfg(windows)]
    #[test]
    fn a_text_file_has_no_icon() {
        let dir = scratch_dir("no_icon");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sample.txt");
        std::fs::write(&p, b"hello").unwrap();
        assert!(!has_icon(&p));
        std::fs::remove_dir_all(&dir).ok();
    }
}
