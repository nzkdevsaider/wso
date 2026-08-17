//! The four command entry points `main` dispatches to. This is the layer
//! that turns the pure reasoning in `plan` and `ops` into console output and
//! (subject to confirmation) real filesystem changes.

use crate::cli::Cli;
use crate::config::{self, ConfigStatus};
use crate::layout::{self, Organization, Target};
use crate::ops;
use crate::plan::{plan, Action};
use crate::platform::{epic::EpicProvider, steam::SteamProvider, Game, GameProvider, Platform};
use std::path::{Path, PathBuf};

/// Loads the config, warning the user if the file on disk was corrupt.
///
/// A missing file is normal on first run: `Config::default()` is exactly
/// right and there is nothing to tell the user. A corrupt file is different
/// — the tool is silently substituting defaults for whatever the user had
/// actually chosen (which target, which organization, which exclusions),
/// and if this run goes on to call `config::save`, those defaults would
/// permanently replace the user's real preferences. The warning names the
/// file, quotes the parse error so the user can see exactly what is wrong,
/// and says plainly that the broken file is not being thrown away: `save`
/// renames it aside to a `.invalid` sibling rather than overwriting it (see
/// `config::save`).
fn load_config_with_warning(path: &Path) -> config::Config {
    let (cfg, status) = config::load(path);
    if let ConfigStatus::Corrupt(err) = status {
        eprintln!(
            "Warning: the configuration file at {} exists but could not be parsed, so \
             defaults are being used for this run instead. Parse error: \"{err}\".\n\
             Your file has not been deleted: the next time preferences are saved, this \
             broken file will be preserved as a sibling (\"<name>.invalid\", or \
             \".invalid.2\" and so on if that name is already taken) rather than being \
             overwritten.",
            path.display()
        );
    }
    cfg
}

/// Detects installed platforms and lists their games.
///
/// A platform the user has not installed is informational, not an error —
/// most users have only one of Steam or Epic, so this is the common case,
/// not an edge case. A platform that *is* installed but whose data could
/// not be read (a corrupt or unreadable `libraryfolders.vdf`, a manifest
/// directory that vanished mid-scan, ...) is different: the resulting game
/// list would be silently incomplete, and `clean`'s sibling command,
/// `sync`, could then delete shortcuts for games that are actually still
/// installed simply because this scan failed to see them. That is exactly
/// the "unreadable Steam installation" the global exit-code contract calls
/// out as a code-2 failure, so it is surfaced as `Err` here rather than as
/// another note, and the caller must abort the whole run rather than
/// proceed on partial data.
fn collect_games(cfg: &config::Config) -> Result<(Vec<Game>, Vec<String>), String> {
    let mut games = Vec::new();
    let mut notes = Vec::new();

    if cfg.platforms.contains(&Platform::Steam) {
        match SteamProvider::detect() {
            None => notes.push("Steam is not installed; skipping.".to_string()),
            Some(p) => match p.games() {
                Ok(mut g) => games.append(&mut g),
                Err(e) => return Err(format!("Steam is installed but could not be read: {e}")),
            },
        }
    }
    if cfg.platforms.contains(&Platform::Epic) {
        match EpicProvider::detect() {
            None => notes.push("Epic Games Launcher is not installed; skipping.".to_string()),
            Some(p) => match p.games() {
                Ok(mut g) => games.append(&mut g),
                Err(e) => return Err(format!("Epic is installed but could not be read: {e}")),
            },
        }
    }
    Ok((games, notes))
}

fn describe(actions: &[Action]) {
    for a in actions {
        match a {
            Action::Create(d) => println!("  create    {}", d.path.display()),
            Action::Retarget { path, .. } => println!("  retarget  {}", path.display()),
            Action::FixIcon { path, .. } => println!("  fix icon  {}", path.display()),
            Action::Move { from, to, .. } => {
                println!("  move      {} -> {}", from.display(), to.display())
            }
            Action::DeleteOrphan(p) => println!("  delete    {}", p.display()),
            Action::Skip { .. } => {}
        }
    }
}

fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    inquire::Confirm::new(prompt)
        .with_default(false)
        .prompt()
        .unwrap_or(false)
}

fn exit_code(failed: usize) -> i32 {
    if failed > 0 {
        1
    } else {
        0
    }
}

/// Reports what `ops::apply` did — or, on a dry run, what it would have
/// done. `ops::apply` labels a dry-run action exactly as it would label the
/// real one (a would-be `Create` still summarizes as `created`), so this is
/// the one place that must not repeat those words unqualified: printing
/// "9 created" after a `--dry-run` would flatly contradict the tool's own
/// promise, in its `--help` text, that a dry run never modifies a shortcut.
fn print_summary(s: &ops::Summary, dry_run: bool) {
    if dry_run {
        println!(
            "  {} would be created, {} would be updated, {} would be moved, \
             {} would be deleted, {} unchanged, {} failed",
            s.created, s.updated, s.moved, s.deleted, s.skipped, s.failed
        );
    } else {
        println!(
            "  {} created, {} updated, {} moved, {} deleted, {} unchanged, {} failed",
            s.created, s.updated, s.moved, s.deleted, s.skipped, s.failed
        );
    }
}

/// Prints the pending actions, asks for confirmation unless `assume_yes` or
/// there is nothing to do, applies them, and reports any per-item failures.
///
/// This is the single choke point every destructive action passes through,
/// for both `sync` and `clean`: a user who answers "no" here has nothing
/// changed, because `ops::apply` is only ever reached after this function
/// has already decided to proceed (or the batch was empty to begin with, in
/// which case `apply` is still called, but only with a dry-run flag so it
/// can never touch anything real).
fn apply_with_confirmation(
    heading: &str,
    actions: &[Action],
    dry_run: bool,
    assume_yes: bool,
) -> ops::Summary {
    let pending: Vec<&Action> =
        actions.iter().filter(|a| !matches!(a, Action::Skip { .. })).collect();

    println!("\n{heading} — {} change(s) pending", pending.len());
    describe(actions);

    if pending.is_empty() {
        return ops::summarize(&ops::apply(actions, true));
    }
    if !dry_run && !confirm("Apply these changes?", assume_yes) {
        println!("Cancelled.");
        return ops::Summary::default();
    }

    let outcomes = ops::apply(actions, dry_run);
    for o in outcomes.iter().filter(|o| o.result.is_err()) {
        eprintln!("  failed to {} {}: {}", o.action, o.name, o.result.as_ref().unwrap_err());
    }
    ops::summarize(&outcomes)
}

/// Reconciles and (subject to confirmation) applies changes for one sync
/// target under the configured organization.
fn sync_target(
    cfg: &config::Config,
    games: &[Game],
    target: Target,
    dry_run: bool,
    assume_yes: bool,
) -> Result<ops::Summary, String> {
    let base = layout::target_dir(target).map_err(|e| e.to_string())?;
    let cache = config::cache_dir();

    let owned = ops::scan_owned(&ops::managed_dirs(&base, &cfg.organization));
    let desired = ops::build_desired(games, cfg, &base, &cache);
    let actions = plan(&desired, &owned);

    Ok(apply_with_confirmation(target.label(), &actions, dry_run, assume_yes))
}

pub fn sync(cli: &Cli) -> i32 {
    let path = config::config_path();
    let mut cfg = load_config_with_warning(&path);

    if !cli.is_non_interactive() {
        if crate::wizard::run(&mut cfg).is_err() {
            println!("Cancelled.");
            return 0;
        }
    }
    cli.apply_overrides(&mut cfg);

    let (games, notes) = match collect_games(&cfg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };
    for n in &notes {
        println!("{n}");
    }
    println!("{} game(s) detected.", games.len());

    let mut failed = 0;
    for target in cfg.targets.iter().copied() {
        match sync_target(&cfg, &games, target, cli.dry_run, cli.yes) {
            Ok(s) => {
                print_summary(&s, cli.dry_run);
                failed += s.failed;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        }
    }

    // Never on a dry run (the user asked what would happen, not to be
    // reconfigured — see the module-level note on `--dry-run`), and never
    // before this point, so a wizard cancellation above returns without
    // writing anything either.
    if !cli.dry_run {
        if let Err(e) = config::save(&path, &cfg) {
            eprintln!("Warning: could not save preferences: {e}");
        }
    }
    exit_code(failed)
}

/// Every organization variant `sync` is able to produce, purely for the
/// purpose of deciding where `clean` should look. `clean` cannot rely on
/// the *current* configuration alone: a user who once organized into a
/// unified "Games" folder and has since switched to per-platform folders
/// (or renamed the folder) would have shortcuts sitting in the old layout
/// that a scan of only today's configuration could never find, even though
/// the command advertises removing every shortcut this tool ever created.
/// Since `Unified` and `PerPlatform` both key off an arbitrary,
/// user-chosen folder name, this can only be a best-effort list: the name
/// currently configured (if any), plus the shared default `"Games"`. A
/// shortcut sitting under some third, no-longer-configured folder name is
/// still not found — but that is not a safety gap, only a coverage one:
/// ownership signatures (`shortcut::signature`) gate every deletion
/// regardless of which directory a file sits in, so widening this scan can
/// never make an unrelated, unsigned shortcut eligible for deletion.
fn candidate_organizations(cfg: &config::Config) -> Vec<Organization> {
    let mut folder_names = vec!["Games".to_string()];
    let configured = match &cfg.organization {
        Organization::Unified { folder } => Some(folder.clone()),
        Organization::PerPlatform { parent: Some(parent) } => Some(parent.clone()),
        Organization::PerPlatform { parent: None } | Organization::Flat => None,
    };
    if let Some(name) = configured {
        if !folder_names.contains(&name) {
            folder_names.push(name);
        }
    }

    let mut orgs = vec![Organization::Flat, Organization::PerPlatform { parent: None }];
    for name in folder_names {
        orgs.push(Organization::Unified { folder: name.clone() });
        orgs.push(Organization::PerPlatform { parent: Some(name) });
    }
    orgs
}

/// Every directory `clean` scans: both known-folder bases (Desktop and the
/// per-user Start Menu — `clean` is not limited to whichever targets happen
/// to be configured right now, since the whole point is to find shortcuts
/// left behind by a *previous* configuration), crossed with every
/// organization layout in `candidate_organizations`. Duplicate directories
/// (e.g. `PerPlatform { parent: None }` and `PerPlatform { parent: Some("Games") }`
/// sharing a `Steam` subfolder only when "Games" also happens to be a
/// platform display name, which it never is) are still de-duplicated
/// defensively so `scan_owned` never reads the same directory twice.
fn clean_scan_dirs(cfg: &config::Config) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for target in [Target::Desktop, Target::StartMenu] {
        let Ok(base) = layout::target_dir(target) else {
            continue;
        };
        for org in candidate_organizations(cfg) {
            for d in layout::managed_dirs(&base, &org) {
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
    }
    dirs
}

pub fn clean(cli: &Cli) -> i32 {
    println!(
        "This removes every shortcut WinShortcutsOrganizer created, across every layout \
         it can produce (flat, unified, and per-platform folders, under both the Desktop \
         and the Start Menu) — not just the layout currently configured — so shortcuts \
         left behind by a previous organization mode are found too."
    );
    println!("Shortcuts you made yourself, or that came from anywhere else, are never touched.");

    let path = config::config_path();
    let cfg = load_config_with_warning(&path);

    let dirs = clean_scan_dirs(&cfg);
    let owned = ops::scan_owned(&dirs);
    let actions = plan(&[], &owned);

    let summary = apply_with_confirmation("Every managed layout", &actions, cli.dry_run, cli.yes);
    print_summary(&summary, cli.dry_run);
    exit_code(summary.failed)
}

pub fn status(cli: &Cli) -> i32 {
    let path = config::config_path();
    let mut cfg = load_config_with_warning(&path);
    cli.apply_overrides(&mut cfg);

    let (games, notes) = match collect_games(&cfg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };
    for n in &notes {
        println!("{n}");
    }
    println!("{} game(s) detected.", games.len());

    let cache = config::cache_dir();
    for target in &cfg.targets {
        let Ok(base) = layout::target_dir(*target) else {
            continue;
        };
        let owned = ops::scan_owned(&ops::managed_dirs(&base, &cfg.organization));
        let desired = ops::build_desired(&games, &cfg, &base, &cache);
        let actions = plan(&desired, &owned);
        let pending = actions.iter().filter(|a| !matches!(a, Action::Skip { .. })).count();
        println!(
            "\n{}: {} managed shortcut(s), {} out of sync",
            target.label(),
            owned.len(),
            pending
        );
        describe(&actions);
    }
    0
}

pub fn migrate(_cli: &Cli) -> i32 {
    println!("migrate is implemented in Task 15");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `candidate_organizations` must include the layout the user is
    /// currently configured to use — otherwise a plain `clean` run right
    /// after a normal `sync` could miss the very shortcuts that sync just
    /// created, which would be a much more common failure than the
    /// stale-layout scenario this function primarily exists for.
    #[test]
    fn candidate_organizations_always_includes_the_currently_configured_layout() {
        let mut cfg = config::Config::default();
        cfg.organization = Organization::PerPlatform { parent: Some("Juegos".into()) };

        let orgs = candidate_organizations(&cfg);
        assert!(
            orgs.contains(&Organization::PerPlatform { parent: Some("Juegos".into()) }),
            "expected the configured PerPlatform(\"Juegos\") layout among {orgs:?}"
        );
    }

    /// The whole point of correction 1: a user who *used to* organize into
    /// a unified "Games" folder, and is now configured for something else
    /// entirely, must still have "Games" scanned so old shortcuts are not
    /// stranded. This is the scenario a scan of only the current config
    /// cannot detect.
    #[test]
    fn candidate_organizations_includes_the_default_games_folder_even_when_not_configured() {
        let mut cfg = config::Config::default();
        cfg.organization = Organization::PerPlatform { parent: Some("Juegos".into()) };

        let orgs = candidate_organizations(&cfg);
        assert!(
            orgs.contains(&Organization::Unified { folder: "Games".into() }),
            "expected the default Unified(\"Games\") layout among {orgs:?}"
        );
        assert!(
            orgs.contains(&Organization::PerPlatform { parent: Some("Games".into()) }),
            "expected the default PerPlatform(\"Games\") layout among {orgs:?}"
        );
    }

    /// Flat mode and parent-less per-platform mode do not depend on any
    /// folder name at all, so they must always be scanned regardless of
    /// what is currently configured.
    #[test]
    fn candidate_organizations_always_includes_flat_and_parentless_per_platform() {
        let cfg = config::Config::default();
        let orgs = candidate_organizations(&cfg);
        assert!(orgs.contains(&Organization::Flat));
        assert!(orgs.contains(&Organization::PerPlatform { parent: None }));
    }

    /// When the configured folder name already *is* "Games" (the shared
    /// default), the list must not contain two identical
    /// `Unified { folder: "Games" }` entries — proving the de-duplication
    /// keys on the folder name, not merely on "is this the default".
    #[test]
    fn candidate_organizations_does_not_duplicate_when_configured_name_is_the_default() {
        let mut cfg = config::Config::default();
        cfg.organization = Organization::Unified { folder: "Games".into() };

        let orgs = candidate_organizations(&cfg);
        let games_unified_count = orgs
            .iter()
            .filter(|o| matches!(o, Organization::Unified { folder } if folder == "Games"))
            .count();
        assert_eq!(
            games_unified_count, 1,
            "expected exactly one Unified(\"Games\") entry, found {games_unified_count} in {orgs:?}"
        );
    }
}
