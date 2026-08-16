pub mod steam;
pub mod vdf;

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Steam,
    Epic,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Steam => "steam",
            Platform::Epic => "epic",
        }
    }

    pub fn from_str_tag(tag: &str) -> Option<Platform> {
        match tag {
            "steam" => Some(Platform::Steam),
            "epic" => Some(Platform::Epic),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Platform::Steam => "Steam",
            Platform::Epic => "Epic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: String,
    pub working_dir: Option<PathBuf>,
}

/// Ordered sources a shortcut icon may come from. The resolver takes the
/// first that validates, so providers only declare possibilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconCandidate {
    /// A file Windows can render an icon from directly (.exe, .ico, .dll).
    Executable(PathBuf),
    /// A raster image that must be converted to .ico first.
    ImageFile(PathBuf),
    /// Last resort, always assumed to work.
    Fallback(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub platform: Platform,
    pub id: String,
    pub name: String,
    pub install_dir: PathBuf,
    pub launch: Launch,
    pub icon_candidates: Vec<IconCandidate>,
}

#[derive(Debug)]
pub enum ProviderError {
    NotInstalled,
    Unreadable { path: PathBuf, detail: String },
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::NotInstalled => write!(f, "platform is not installed"),
            ProviderError::Unreadable { path, detail } => {
                write!(f, "could not read {}: {detail}", path.display())
            }
        }
    }
}

impl std::error::Error for ProviderError {}

pub trait GameProvider {
    fn platform(&self) -> Platform;
    fn games(&self) -> Result<Vec<Game>, ProviderError>;
}
