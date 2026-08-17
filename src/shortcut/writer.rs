use super::LnkData;
use std::path::{Path, PathBuf};

/// Length of the UTF-16 buffers used to read strings back out of an
/// `IShellLinkW`. `MAX_PATH` (260) is what a naive implementation reaches
/// for, but a Steam library nested a few directories deep combined with a
/// long game title can get close to that limit, and shell links are not
/// required to stay under it. 4096 UTF-16 code units (8 KiB per buffer)
/// comfortably covers real-world deep install paths and long argument/URI
/// strings (see the Epic URI round-trip test) while staying small enough
/// to allocate per call without worrying about thread stack limits — the
/// buffers are heap-allocated `Vec<u16>`, not stack arrays, specifically so
/// this size choice never risks a stack overflow.
const BUF_LEN: usize = 4096;

#[derive(Debug)]
pub enum ShortcutError {
    Com(String),
    Io(String),
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShortcutError::Com(d) => write!(f, "shell link error: {d}"),
            ShortcutError::Io(d) => write!(f, "file error: {d}"),
        }
    }
}

impl std::error::Error for ShortcutError {}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    /// COM must be initialized on the calling thread before any
    /// `IShellLinkW`/`IPersistFile` call, and uninitialized again when that
    /// thread is done with it. This guard owns exactly one thread's
    /// init/uninit pair — construct one per thread that calls `write_lnk`
    /// or `read_lnk` (each `#[test]` function runs on its own thread under
    /// the default test harness, so each test constructs its own `ComGuard`
    /// rather than sharing one across threads).
    pub struct ComGuard;

    impl ComGuard {
        pub fn new() -> Result<ComGuard, ShortcutError> {
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|e| ShortcutError::Com(e.to_string()))?;
            }
            Ok(ComGuard)
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn buf_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    pub fn write_lnk(path: &Path, data: &LnkData) -> Result<(), ShortcutError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ShortcutError::Io(e.to_string()))?;
        }
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            link.SetPath(&HSTRING::from(data.target.as_os_str()))
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
            link.SetArguments(&HSTRING::from(data.args.as_str()))
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
            link.SetDescription(&HSTRING::from(data.description.as_str()))
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            if let Some(wd) = &data.working_dir {
                link.SetWorkingDirectory(&HSTRING::from(wd.as_os_str()))
                    .map_err(|e| ShortcutError::Com(e.to_string()))?;
            }
            if let Some((icon_path, index)) = &data.icon {
                link.SetIconLocation(&HSTRING::from(icon_path.as_os_str()), *index)
                    .map_err(|e| ShortcutError::Com(e.to_string()))?;
            }

            let persist: IPersistFile =
                link.cast().map_err(|e| ShortcutError::Com(e.to_string()))?;
            persist
                .Save(&HSTRING::from(path.as_os_str()), true)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
        }
        Ok(())
    }

    pub fn read_lnk(path: &Path) -> Result<LnkData, ShortcutError> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
            let persist: IPersistFile =
                link.cast().map_err(|e| ShortcutError::Com(e.to_string()))?;
            persist
                .Load(&HSTRING::from(path.as_os_str()), STGM_READ)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut target = vec![0u16; BUF_LEN];
            link.GetPath(&mut target, std::ptr::null_mut::<WIN32_FIND_DATAW>(), 0)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut args = vec![0u16; BUF_LEN];
            link.GetArguments(&mut args)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut desc = vec![0u16; BUF_LEN];
            link.GetDescription(&mut desc)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut wd = vec![0u16; BUF_LEN];
            link.GetWorkingDirectory(&mut wd)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut icon = vec![0u16; BUF_LEN];
            let mut index = 0i32;
            link.GetIconLocation(&mut icon, &mut index)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let icon_str = buf_to_string(&icon);
            let wd_str = buf_to_string(&wd);

            Ok(LnkData {
                target: PathBuf::from(buf_to_string(&target)),
                args: buf_to_string(&args),
                working_dir: if wd_str.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(wd_str))
                },
                icon: if icon_str.is_empty() {
                    None
                } else {
                    Some((PathBuf::from(icon_str), index))
                },
                description: buf_to_string(&desc),
            })
        }
    }
}

#[cfg(windows)]
pub use imp::{read_lnk, write_lnk, ComGuard};

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct ComGuard;

    impl ComGuard {
        pub fn new() -> Result<ComGuard, ShortcutError> {
            Err(ShortcutError::Com("shortcuts require Windows".into()))
        }
    }

    pub fn write_lnk(_path: &Path, _data: &LnkData) -> Result<(), ShortcutError> {
        Err(ShortcutError::Com("shortcuts require Windows".into()))
    }

    pub fn read_lnk(_path: &Path) -> Result<LnkData, ShortcutError> {
        Err(ShortcutError::Com("shortcuts require Windows".into()))
    }
}

#[cfg(not(windows))]
pub use imp::{read_lnk, write_lnk, ComGuard};
