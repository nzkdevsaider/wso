use crate::config::Config;
use crate::layout::{Organization, Target};
use crate::platform::Platform;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Create and update shortcuts for installed games
    Sync,
    /// Convert Steam's .url shortcuts into real .lnk shortcuts
    Migrate,
    /// Remove every shortcut this tool created
    Clean,
    /// Show what exists and what is out of sync
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OrganizeArg {
    Flat,
    Unified,
    PerPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TargetArg {
    Desktop,
    StartMenu,
    Both,
}

#[derive(Debug, Parser)]
#[command(
    name = "wso",
    version = crate::banner::VERSION,
    before_help = crate::banner::header(),
    about = "Organize Steam and Epic game shortcuts on Windows",
    long_about = "Creates real .lnk shortcuts with correct icons for your installed \
Steam and Epic games, on the Desktop and in the Start Menu.\n\n\
Note: Windows has not allowed programs to pin items to the Start menu panel \
since Windows 10 version 1607. Shortcuts placed in the Start Menu's Programs \
folder appear under All apps and are searchable there, but pinning a shortcut \
to the Start panel or the taskbar remains a manual right-click action for the \
user; this tool cannot do it for you and does not try.\n\n\
--dry-run shows what would happen without modifying any of your shortcuts. \
It does still populate this tool's own icon cache under %LOCALAPPDATA%, \
because resolving icons happens before any shortcut is written; your Desktop \
and Start Menu are left untouched either way."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Where to place shortcuts
    #[arg(long, value_enum, global = true)]
    pub target: Option<TargetArg>,

    /// How to group shortcuts
    #[arg(long, value_enum, global = true)]
    pub organize: Option<OrganizeArg>,

    /// Folder name for unified or per-platform mode
    #[arg(long, global = true)]
    pub folder: Option<String>,

    /// Comma-separated platforms, e.g. steam,epic
    #[arg(long, value_delimiter = ',', global = true)]
    pub platforms: Option<Vec<String>>,

    /// Show what would happen without modifying any of your shortcuts
    /// (this tool's own icon cache may still be populated; see --help)
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Skip confirmation prompts
    #[arg(long, short, global = true)]
    pub yes: bool,
}

impl Cli {
    pub fn effective_command(&self) -> Command {
        self.command.unwrap_or(Command::Sync)
    }

    /// Whether the invocation carried enough flags to skip the wizard.
    pub fn is_non_interactive(&self) -> bool {
        self.yes || self.target.is_some() || self.organize.is_some()
    }

    pub fn apply_overrides(&self, cfg: &mut Config) {
        if let Some(t) = self.target {
            cfg.targets = match t {
                TargetArg::Desktop => vec![Target::Desktop],
                TargetArg::StartMenu => vec![Target::StartMenu],
                TargetArg::Both => vec![Target::Desktop, Target::StartMenu],
            };
        }
        if let Some(o) = self.organize {
            let folder = self.folder.clone().unwrap_or_else(|| "Games".into());
            cfg.organization = match o {
                OrganizeArg::Flat => Organization::Flat,
                OrganizeArg::Unified => Organization::Unified { folder },
                OrganizeArg::PerPlatform => Organization::PerPlatform {
                    parent: self.folder.clone(),
                },
            };
        } else if let Some(folder) = &self.folder {
            // A folder name on its own renames the existing unified folder.
            if let Organization::Unified { .. } = cfg.organization {
                cfg.organization = Organization::Unified {
                    folder: folder.clone(),
                };
            }
        }
        if let Some(names) = &self.platforms {
            let selected: Vec<Platform> = names
                .iter()
                .filter_map(|n| Platform::from_str_tag(&n.to_ascii_lowercase()))
                .collect();
            if !selected.is_empty() {
                cfg.platforms = selected;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn no_arguments_means_sync() {
        let cli = Cli::parse_from(["wso"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.effective_command(), Command::Sync);
    }

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            Cli::parse_from(["wso", "sync"]).effective_command(),
            Command::Sync
        );
        assert_eq!(
            Cli::parse_from(["wso", "clean"]).effective_command(),
            Command::Clean
        );
        assert_eq!(
            Cli::parse_from(["wso", "status"]).effective_command(),
            Command::Status
        );
        assert_eq!(
            Cli::parse_from(["wso", "migrate"]).effective_command(),
            Command::Migrate
        );
    }

    #[test]
    fn parses_organization_flags() {
        let cli = Cli::parse_from([
            "wso",
            "sync",
            "--organize",
            "per-platform",
            "--folder",
            "Juegos",
            "--yes",
        ]);
        assert_eq!(cli.organize, Some(OrganizeArg::PerPlatform));
        assert_eq!(cli.folder.as_deref(), Some("Juegos"));
        assert!(cli.yes);
        assert!(!cli.dry_run);
    }

    #[test]
    fn flags_override_config() {
        let cli = Cli::parse_from([
            "wso",
            "--target",
            "desktop",
            "--organize",
            "unified",
            "--folder",
            "Games",
        ]);
        let mut cfg = crate::config::Config::default();
        cli.apply_overrides(&mut cfg);
        assert_eq!(cfg.targets, vec![crate::layout::Target::Desktop]);
        assert_eq!(
            cfg.organization,
            crate::layout::Organization::Unified {
                folder: "Games".into()
            }
        );
    }

    #[test]
    fn absent_flags_leave_config_untouched() {
        let cli = Cli::parse_from(["wso"]);
        let mut cfg = crate::config::Config::default();
        let before = cfg.clone();
        cli.apply_overrides(&mut cfg);
        assert_eq!(cfg, before);
    }

    #[test]
    fn parses_platform_selection() {
        let cli = Cli::parse_from(["wso", "--platforms", "steam"]);
        let mut cfg = crate::config::Config::default();
        cli.apply_overrides(&mut cfg);
        assert_eq!(cfg.platforms, vec![crate::platform::Platform::Steam]);
    }
}
