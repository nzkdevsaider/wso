pub mod signature;
pub mod url_legacy;
pub mod writer;

use std::path::PathBuf;

/// The subset of a Windows shortcut this tool reads and writes. `icon` is
/// deliberately independent of `target`: that separation is what lets a
/// shortcut launch through Steam while showing the game's own icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LnkData {
    pub target: PathBuf,
    pub args: String,
    pub working_dir: Option<PathBuf>,
    pub icon: Option<(PathBuf, i32)>,
    pub description: String,
}
