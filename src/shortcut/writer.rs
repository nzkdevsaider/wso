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
#[cfg(windows)]
const BUF_LEN: usize = 4096;

/// Measured, not assumed: writing descriptions of increasing length into
/// isolated `.lnk` files (one file per length, to rule out contamination
/// between attempts) showed a description of exactly 260 UTF-16 code
/// units survives a save/load cycle intact, and 261 is silently truncated
/// back down to 260 — even against a 100,000-unit read buffer, so this is
/// `IShellLinkW::SetDescription` truncating at write time (apparently to
/// `MAX_PATH`, despite the interface being the "wide" one), not a limit of
/// anything this crate controls. There is no buffer size on the read side
/// that would fix this; the string is already short by the time it is
/// saved to disk. See `writes_and_reads_back_a_description_at_the_measured_shell_link_limit`
/// and its rejecting sibling in `tests/lnk_roundtrip.rs` for the boundary
/// this constant is pinned to.
///
/// Not `#[cfg(windows)]`-gated, unlike its siblings: `shortcut::signature`
/// (compiled on every platform) budgets a game title against this same
/// constant when composing a description, so that the value it produces
/// never trips `check_round_trippable` below. There is exactly one number
/// for this limit in the crate; `signature::encode` and `write_lnk` must
/// never be allowed to drift apart on it.
pub(crate) const DESCRIPTION_LIMIT_UTF16: usize = 260;

/// Measured: unlike descriptions, arguments showed no write-time cap at
/// all — a 32,000-character argument string round-tripped correctly given
/// a large enough read buffer. The binding constraint for arguments is
/// therefore `BUF_LEN`, not anything Windows imposes. `GetArguments` needs
/// one code unit of headroom in the buffer to null-terminate: at
/// `BUF_LEN` exactly, a `BUF_LEN`-length argument string comes back
/// truncated to `BUF_LEN - 1`; at `BUF_LEN - 1` it round-trips exactly.
#[cfg(windows)]
const ARGS_LIMIT_UTF16: usize = BUF_LEN - 1;

#[derive(Debug)]
pub enum ShortcutError {
    Com(String),
    Io(String),
    /// A field would not survive a write-then-read round trip intact.
    /// `write_lnk` checks this before touching the filesystem or COM, so
    /// that this crate's guarantee holds: anything it writes, it can read
    /// back byte-for-byte. `limit` is expressed in UTF-16 code units,
    /// matching the same unit `field`'s length is measured in.
    FieldTooLong {
        field: &'static str,
        len: usize,
        limit: usize,
    },
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShortcutError::Com(d) => write!(f, "shell link error: {d}"),
            ShortcutError::Io(d) => write!(f, "file error: {d}"),
            ShortcutError::FieldTooLong { field, len, limit } => write!(
                f,
                "{field} is {len} UTF-16 code units long, which exceeds the {limit}-unit limit a shell link can round-trip"
            ),
        }
    }
}

impl std::error::Error for ShortcutError {}

/// Rejects a field before any file or COM object is touched if it cannot
/// survive a write-then-read round trip intact, per the limits measured
/// above. Length is counted in UTF-16 code units — the unit both the
/// `IShellLinkW` API and `DESCRIPTION_LIMIT_UTF16`/`ARGS_LIMIT_UTF16` are
/// expressed in — not bytes or `char`s, so multi-byte UTF-8 text is
/// measured the same way Windows measures it.
#[cfg(windows)]
fn check_round_trippable(
    field: &'static str,
    value: &str,
    limit: usize,
) -> Result<(), ShortcutError> {
    let len = value.encode_utf16().count();
    if len > limit {
        Err(ShortcutError::FieldTooLong { field, len, limit })
    } else {
        Ok(())
    }
}

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
        check_round_trippable("description", &data.description, DESCRIPTION_LIMIT_UTF16)?;
        check_round_trippable("args", &data.args, ARGS_LIMIT_UTF16)?;

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

    /// Reads every field of a `.lnk` back out, including shortcuts this
    /// tool did not create. `description` and `args` are read into a
    /// `BUF_LEN`-unit buffer; a foreign shortcut whose value is longer than
    /// that buffer comes back silently truncated by the underlying Win32
    /// call rather than erroring, because `Get*` has no way to report "the
    /// value didn't fit" short of failing the whole read.
    ///
    /// This is a deliberate asymmetry, not an oversight: `write_lnk`
    /// enforces the round-trippable limits up front, so nothing this tool
    /// writes can ever come back truncated. A foreign shortcut's
    /// description is only ever compared against the `[wso:...]` ownership
    /// marker (see `shortcut::signature`), which sits at the *end* of the
    /// string — truncating a foreign description can only ever strip that
    /// marker if it happened to be present, which makes the shortcut read
    /// as *not ours*. That is the safe direction: this tool must never
    /// treat a foreign shortcut as its own, and a truncated foreign
    /// description can only push the outcome towards "not ours," never
    /// towards "ours."
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
