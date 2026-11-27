//! The four command entry points `main` dispatches to. This is the layer
//! that turns the pure reasoning in `plan` and `ops` into console output and
//! (subject to confirmation) real filesystem changes.
//!
//! Exit codes are deliberately narrow: `0` means every item succeeded, `1`
//! means some item failed (a platform that could not be read, a target
//! folder that could not be resolved, a shortcut that could not be
//! written), and `2` is reserved for the tool provably being unable to act
//! at all — which, in this program, happens before any command in this
//! module runs at all (`ComGuard::new()` failing in `main`). None of the
//! functions below ever return `2`: a partial problem is always a `1`, not
//! an excuse to abandon everything else that might still succeed.

use crate::cli::Cli;
use crate::config::{self, ConfigStatus};
use crate::layout::{self, LayoutError, Target};
use crate::ops;
use crate::plan::{Action, plan};
use crate::platform::{Game, GameProvider, Platform, epic::EpicProvider, steam::SteamProvider};
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

/// What came of trying to detect and list one platform's games: `None` when
/// the platform was never installed at all, `Some(Ok(games))` when it was
/// installed and read successfully, `Some(Err(detail))` when it was
/// installed but its data could not be read.
type PlatformOutcome = Option<Result<Vec<Game>, String>>;

/// Every game found across every selected platform, plus the messages
/// worth showing the user split by severity.
struct Detection {
    games: Vec<Game>,
    /// A platform simply was not installed. The common case — most users
    /// have only one of Steam or Epic — so this is informational, not a
    /// problem.
    infos: Vec<String>,
    /// A platform *was* detected as installed but its data could not be
    /// read (a corrupt or unreadable `libraryfolders.vdf`, a manifest
    /// directory that vanished mid-scan, ...). This used to be treated as
    /// a fatal, whole-run error, which was wrong in a very concrete way:
    /// `collect_games` reads Steam before Epic, so a user with a healthy
    /// Steam library and only a broken Epic install would have their Steam
    /// games detected, then thrown away, then be told the run failed
    /// globally — doing real work and discarding it to report a failure
    /// that was never global to begin with. It is now folded in here as a
    /// per-platform failed item instead: the other platform's games are
    /// still collected and acted on, and this only ever contributes to
    /// exit code `1`.
    failures: Vec<String>,
}

/// Folds one platform's detection `outcome` into the running `games`,
/// `infos` and `failures` lists. Pulled out of `collect_games` specifically
/// so this — the actual "not installed is fine, unreadable is a failed
/// item, not an abort" decision — is unit-testable without needing a real
/// Steam or Epic installation, let alone a broken one.
fn record_platform(
    outcome: PlatformOutcome,
    not_installed_note: &str,
    unreadable_prefix: &str,
    games: &mut Vec<Game>,
    infos: &mut Vec<String>,
    failures: &mut Vec<String>,
) {
    match outcome {
        None => infos.push(not_installed_note.to_string()),
        Some(Ok(mut g)) => games.append(&mut g),
        Some(Err(detail)) => failures.push(format!("{unreadable_prefix}: {detail}")),
    }
}

/// Detects installed platforms and lists their games. See `Detection` for
/// how "not installed" and "installed but unreadable" are told apart.
fn collect_games(cfg: &config::Config) -> Detection {
    let mut games = Vec::new();
    let mut infos = Vec::new();
    let mut failures = Vec::new();

    if cfg.platforms.contains(&Platform::Steam) {
        let outcome = SteamProvider::detect().map(|p| p.games().map_err(|e| e.to_string()));
        record_platform(
            outcome,
            "Steam is not installed; skipping.",
            "Steam is installed but could not be read",
            &mut games,
            &mut infos,
            &mut failures,
        );
    }
    if cfg.platforms.contains(&Platform::Epic) {
        let outcome = EpicProvider::detect().map(|p| p.games().map_err(|e| e.to_string()));
        record_platform(
            outcome,
            "Epic Games Launcher is not installed; skipping.",
            "Epic is installed but could not be read",
            &mut games,
            &mut infos,
            &mut failures,
        );
    }

    Detection {
        games,
        infos,
        failures,
    }
}

/// Prints `detection`'s notes (informational to stdout, failures as
/// warnings to stderr) and the game count, returning the games and the
/// number of platform-level failures so the caller can fold it into its own
/// exit-code accounting.
fn report_detection(detection: Detection) -> (Vec<Game>, usize) {
    for n in &detection.infos {
        println!("{n}");
    }
    for f in &detection.failures {
        eprintln!("Warning: {f}");
    }
    println!("{} game(s) detected.", detection.games.len());
    (detection.games, detection.failures.len())
}

/// A target paired with the base directory it resolved to.
type ResolvedTarget = (Target, PathBuf);

/// A target that could not be resolved, paired with what went wrong.
type UnresolvedTarget = (Target, String);

/// Splits `targets` into the bases that `resolve` could locate and the ones
/// it could not, pairing each unresolved target with what went wrong.
///
/// Generic over `resolve` (in production, always `layout::target_dir`)
/// specifically so the behavior this function embodies — an unresolved
/// target is named and kept, never silently dropped — is unit-testable: the
/// real `layout::target_dir` only fails via a live Windows known-folder
/// lookup, which this test suite has no way to force, so a test substitutes
/// a resolver that fails deterministically instead.
fn resolve_bases<F>(
    targets: impl IntoIterator<Item = Target>,
    resolve: F,
) -> (Vec<ResolvedTarget>, Vec<UnresolvedTarget>)
where
    F: Fn(Target) -> Result<PathBuf, LayoutError>,
{
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for target in targets {
        match resolve(target) {
            Ok(base) => resolved.push((target, base)),
            Err(e) => unresolved.push((target, e.to_string())),
        }
    }
    (resolved, unresolved)
}

/// Prints a warning naming each unresolved target and why, and returns how
/// many there were — every caller folds this straight into its own failed
/// count, since a target this tool could not even look at is a partial
/// failure, not something to pass over in silence.
fn warn_unresolved(unresolved: &[UnresolvedTarget], action: &str) -> usize {
    for (target, detail) in unresolved {
        eprintln!(
            "Warning: could not resolve the {} folder; it was not {action} ({detail}).",
            target.label()
        );
    }
    unresolved.len()
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

/// `pub(crate)` rather than private: `migrate` (its own module — see
/// `crate::migrate` — kept separate specifically so this file does not grow
/// further) reuses this same confirmation choke point rather than
/// duplicating it.
pub(crate) fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    inquire::Confirm::new(prompt)
        .with_default(false)
        .prompt()
        .unwrap_or(false)
}

/// `0` when nothing failed, `1` otherwise. This function is the only place
/// that decides between the two — it deliberately has no path to `2`; see
/// the module doc comment for where that code comes from instead.
pub(crate) fn exit_code(failed: usize) -> i32 {
    if failed > 0 { 1 } else { 0 }
}

/// The exact wording `print_summary` prints, pulled into its own function
/// so the dry-run-vs-real wording choice is unit-testable without capturing
/// stdout.
fn summary_line(s: &ops::Summary, dry_run: bool) -> String {
    if dry_run {
        format!(
            "  {} would be created, {} would be updated, {} would be moved, \
             {} would be deleted, {} unchanged, {} failed",
            s.created, s.updated, s.moved, s.deleted, s.skipped, s.failed
        )
    } else {
        format!(
            "  {} created, {} updated, {} moved, {} deleted, {} unchanged, {} failed",
            s.created, s.updated, s.moved, s.deleted, s.skipped, s.failed
        )
    }
}

/// Reports what `ops::apply` did — or, on a dry run, what it would have
/// done. `ops::apply` labels a dry-run action exactly as it would label the
/// real one (a would-be `Create` still summarizes as `created`), so this is
/// the one place that must not repeat those words unqualified: printing
/// "9 created" after a `--dry-run` would flatly contradict the tool's own
/// promise, in its `--help` text, that a dry run never modifies a shortcut.
fn print_summary(s: &ops::Summary, dry_run: bool) {
    println!("{}", summary_line(s, dry_run));
}

/// Whether a batch actually went ahead, as distinct from how it turned out.
///
/// `sync` needs this and a `Summary` cannot supply it: a declined batch and a
/// batch with nothing pending both produce all-zero counts, and those two
/// must be told apart - see `should_save_preferences`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// The batch ran, or there was nothing pending so nothing was asked.
    Proceeded,
    /// The user was asked to confirm and answered no.
    Declined,
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
///
/// Returns whether it proceeded alongside the summary, because "the user said
/// no" is not something the counts can express - `sync` used to save the
/// wizard's answers over the user's stored preferences even after they
/// declined, precisely because it could not tell that case from a run with
/// nothing to do.
fn apply_with_confirmation(
    heading: &str,
    actions: &[Action],
    dry_run: bool,
    assume_yes: bool,
) -> (ops::Summary, Applied) {
    let pending: Vec<&Action> = actions
        .iter()
        .filter(|a| !matches!(a, Action::Skip { .. }))
        .collect();

    println!("\n{heading} — {} change(s) pending", pending.len());
    describe(actions);

    if pending.is_empty() {
        return (
            ops::summarize(&ops::apply(actions, true)),
            Applied::Proceeded,
        );
    }
    if !dry_run && !confirm("Apply these changes?", assume_yes) {
        println!("Cancelled.");
        return (ops::Summary::default(), Applied::Declined);
    }

    let outcomes = ops::apply(actions, dry_run);
    for o in outcomes.iter().filter(|o| o.result.is_err()) {
        eprintln!(
            "  failed to {} {}: {}",
            o.action,
            o.name,
            o.result.as_ref().unwrap_err()
        );
    }
    (ops::summarize(&outcomes), Applied::Proceeded)
}

/// Whether this run may write the configuration back to disk.
///
/// Two reasons not to. A dry run was a question, not an instruction, so it
/// must not reconfigure anything. And a user who answered "no" to a
/// confirmation declined this configuration - writing the wizard's answers
/// over their stored preferences anyway would mean the one action they
/// explicitly refused is the one that still happened, and their previous
/// target, organization and folder name are gone.
///
/// A run with nothing pending still saves: the user answered the wizard and
/// there simply was nothing to change, which is exactly how someone adjusts
/// preferences without any shortcut needing to move.
///
/// Declining for *any* target blocks the save, not only declining for all of
/// them. Preferences are one setting shared by every target, so a partial
/// refusal cannot be honored partially - and refusing to write is the
/// recoverable direction: nothing is lost and re-running restores the choice,
/// whereas overwriting preferences the user did not agree to cannot be
/// undone.
///
/// Pulled out as a pure function because the path that reaches it cannot be
/// executed by this test suite at all: it needs a real terminal for `inquire`
/// to prompt on, and a real user to answer "no".
fn should_save_preferences(dry_run: bool, outcomes: &[Applied]) -> bool {
    !dry_run && !outcomes.contains(&Applied::Declined)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenDecision {
    Skip,
    Ask,
    OpenNow,
}

/// Decides whether `sync` should open the destination folder(s) after
/// applying changes. Pulled out as a pure function so the dry-run, declined,
/// `--open`, `--yes`, and interactive branches are unit-testable.
fn should_open_after_sync(
    dry_run: bool,
    assume_yes: bool,
    open_flag: bool,
    outcomes: &[Applied],
) -> OpenDecision {
    if dry_run || outcomes.contains(&Applied::Declined) {
        return OpenDecision::Skip;
    }
    if open_flag {
        return OpenDecision::OpenNow;
    }
    if assume_yes {
        OpenDecision::Skip
    } else {
        OpenDecision::Ask
    }
}

/// Opens each existing destination directory in Windows Explorer and returns
/// how many failed to launch. Non-existent directories are reported as an
/// informational note rather than a failure (for instance, when no games were
/// found for a target).
fn open_destinations(dirs: &[PathBuf]) -> usize {
    let mut failed = 0;
    for dir in dirs {
        if !dir.is_dir() {
            println!("Destination folder does not exist yet: {}", dir.display());
            continue;
        }
        println!("Opening {}", dir.display());
        if let Err(e) = ops::open_folder(dir) {
            eprintln!("Warning: could not open {}: {e}", dir.display());
            failed += 1;
        }
    }
    failed
}

/// Reconciles and (subject to confirmation) applies changes for one sync
/// target under the configured organization. `base` is already resolved by
/// the caller (see `resolve_bases`), so this never fails.
fn sync_target(
    cfg: &config::Config,
    games: &[Game],
    base: &Path,
    target: Target,
    dry_run: bool,
    assume_yes: bool,
) -> (ops::Summary, Applied) {
    let cache = config::cache_dir();

    let owned = ops::scan_owned(&ops::scan_dirs(base));
    let desired = ops::build_desired(games, cfg, base, &cache);
    let actions = plan(&desired, &owned);

    apply_with_confirmation(target.label(), &actions, dry_run, assume_yes)
}

pub fn sync(cli: &Cli) -> i32 {
    let path = config::config_path();
    let mut cfg = load_config_with_warning(&path);

    if !cli.is_non_interactive() && crate::wizard::run(&mut cfg).is_err() {
        println!("Cancelled.");
        return 0;
    }
    cli.apply_overrides(&mut cfg);

    let (games, platform_failures) = report_detection(collect_games(&cfg));
    let mut failed = platform_failures;

    let (resolved, unresolved) = resolve_bases(cfg.targets.iter().copied(), layout::target_dir);
    failed += warn_unresolved(&unresolved, "synced");

    let dest_dirs = layout::collect_destination_dirs(&resolved, &cfg.organization);

    let mut applied = Vec::new();
    for (target, base) in resolved {
        let (s, outcome) = sync_target(&cfg, &games, &base, target, cli.dry_run, cli.yes);
        print_summary(&s, cli.dry_run);
        failed += s.failed;
        applied.push(outcome);
    }

    // Never before this point, so a wizard cancellation above returns without
    // writing anything either. See `should_save_preferences` for the two
    // reasons this run may not be allowed to write at all.
    if should_save_preferences(cli.dry_run, &applied)
        && let Err(e) = config::save(&path, &cfg)
    {
        eprintln!("Warning: could not save preferences: {e}");
    }

    let existing_dests: Vec<PathBuf> = dest_dirs.into_iter().filter(|d| d.is_dir()).collect();
    if !existing_dests.is_empty() {
        let should_open = match should_open_after_sync(cli.dry_run, cli.yes, cli.open, &applied) {
            OpenDecision::Skip => false,
            OpenDecision::OpenNow => true,
            OpenDecision::Ask => confirm("Open destination folder(s) in Explorer?", false),
        };
        if should_open {
            failed += open_destinations(&existing_dests);
        }
    }
    exit_code(failed)
}

/// Every directory `clean` scans: both known-folder bases (Desktop and the
/// per-user Start Menu - `clean` is not limited to whichever targets happen
/// to be configured right now, since the whole point is to find shortcuts
/// left behind by a *previous* configuration), each expanded by
/// `ops::scan_dirs` into the directories a shortcut could actually be sitting
/// in. `ops::scan_dirs` already de-duplicates within one base; the extra
/// check below covers the cross-base case, so `scan_owned` never reads the
/// same directory twice however the two known folders are configured. A base
/// that cannot be resolved is returned separately rather than silently
/// dropped - `clean` must tell the user it did not scan everywhere it
/// promised to, not print a clean summary that quietly covered only part of
/// the disk.
///
/// Takes no configuration at all any more, which is the point: what `clean`
/// looks at is decided by what is on disk, not by what the config file
/// happens to still remember.
fn clean_scan_dirs() -> (Vec<PathBuf>, Vec<UnresolvedTarget>) {
    let (resolved, unresolved) =
        resolve_bases([Target::Desktop, Target::StartMenu], layout::target_dir);

    let mut dirs = Vec::new();
    for (_, base) in &resolved {
        for d in ops::scan_dirs(base) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    (dirs, unresolved)
}

pub fn clean(cli: &Cli) -> i32 {
    println!(
        "This removes every shortcut WinShortcutsOrganizer created, across every layout \
         it can produce (flat, unified, and per-platform folders, under both the Desktop \
         and the Start Menu) — not just the layout currently configured — so shortcuts \
         left behind by a previous organization mode are found too."
    );
    println!("Shortcuts you made yourself, or that came from anywhere else, are never touched.");

    // Loaded purely so a corrupt config file is still reported; `clean`
    // itself takes nothing from it, because where it looks is decided by
    // what is on disk.
    let path = config::config_path();
    let _ = load_config_with_warning(&path);

    let (dirs, unresolved) = clean_scan_dirs();
    let mut failed = warn_unresolved(&unresolved, "scanned");

    let owned = ops::scan_owned(&dirs);
    let actions = plan(&[], &owned);

    // `clean` never writes preferences, so whether the user confirmed is of
    // no further interest here.
    let (summary, _) =
        apply_with_confirmation("Every managed layout", &actions, cli.dry_run, cli.yes);
    print_summary(&summary, cli.dry_run);
    failed += summary.failed;
    exit_code(failed)
}

pub fn status(cli: &Cli) -> i32 {
    let path = config::config_path();
    let mut cfg = load_config_with_warning(&path);
    cli.apply_overrides(&mut cfg);

    let (games, platform_failures) = report_detection(collect_games(&cfg));
    let mut failed = platform_failures;

    let (resolved, unresolved) = resolve_bases(cfg.targets.iter().copied(), layout::target_dir);
    failed += warn_unresolved(&unresolved, "checked");

    let cache = config::cache_dir();
    for (target, base) in resolved {
        let owned = ops::scan_owned(&ops::scan_dirs(&base));
        let desired = ops::build_desired(&games, &cfg, &base, &cache);
        let actions = plan(&desired, &owned);
        let pending = actions
            .iter()
            .filter(|a| !matches!(a, Action::Skip { .. }))
            .count();
        println!(
            "\n{}: {} managed shortcut(s), {} out of sync",
            target.label(),
            owned.len(),
            pending
        );
        describe(&actions);
    }
    exit_code(failed)
}

/// Converts legacy Steam `.url` shortcuts (Steam's own web-bookmark-style
/// shortcuts, which show up with a blank generic icon) into real `.lnk`
/// shortcuts. The actual implementation lives in `crate::migrate` — its own
/// module, kept separate so the orchestration/presentation code already in
/// this file does not grow any further — this is only the dispatch point
/// `main` calls through, matching every other command in this module.
pub fn migrate(cli: &Cli) -> i32 {
    crate::migrate::run(cli)
}

pub fn open(cli: &Cli) -> i32 {
    let path = config::config_path();
    let mut cfg = load_config_with_warning(&path);
    cli.apply_overrides(&mut cfg);

    let (resolved, unresolved) = resolve_bases(cfg.targets.iter().copied(), layout::target_dir);
    let mut failed = warn_unresolved(&unresolved, "opened");

    let dirs = layout::collect_destination_dirs(&resolved, &cfg.organization);
    failed += open_destinations(&dirs);
    exit_code(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{IconCandidate, Launch};

    fn test_game(id: &str, name: &str) -> Game {
        Game {
            platform: Platform::Steam,
            id: id.into(),
            name: name.into(),
            install_dir: PathBuf::from(r"C:\Games\Game"),
            launch: Launch {
                program: PathBuf::from(r"C:\Games\steam.exe"),
                args: format!("-applaunch {id}"),
                working_dir: None,
            },
            icon_candidates: vec![IconCandidate::Fallback(PathBuf::from(
                r"C:\Windows\explorer.exe",
            ))],
        }
    }

    // --- should_save_preferences ---

    /// The defect: `sync` wrote the wizard's answers to disk on any non-dry
    /// run, including one where the user had just answered "no" to the
    /// confirmation. Their stored target, organization and folder name were
    /// replaced by the very configuration they had declined - the one action
    /// they explicitly refused being the only one that still happened.
    #[test]
    fn preferences_are_not_saved_when_the_user_declined_the_confirmation() {
        assert!(!should_save_preferences(false, &[Applied::Declined]));
    }

    /// The ordinary interactive run: the user answered the wizard, confirmed
    /// the changes, and their answers are theirs to keep.
    #[test]
    fn preferences_are_saved_when_the_user_confirmed() {
        assert!(should_save_preferences(false, &[Applied::Proceeded]));
    }

    /// A run with nothing pending never asks anything, and must still save:
    /// that is exactly how someone changes preferences when no shortcut
    /// happens to need moving. `Applied` exists to keep this case
    /// distinguishable from a decline, which a `Summary` alone cannot do
    /// because both produce all-zero counts.
    #[test]
    fn preferences_are_saved_when_there_was_nothing_to_confirm() {
        assert!(should_save_preferences(false, &[]));
        assert!(should_save_preferences(false, &[Applied::Proceeded]));
    }

    /// A dry run was a question, not an instruction. It must not reconfigure
    /// anything, however the confirmations went - including the case where
    /// nothing was asked at all.
    #[test]
    fn a_dry_run_never_saves_preferences() {
        assert!(!should_save_preferences(true, &[]));
        assert!(!should_save_preferences(true, &[Applied::Proceeded]));
        assert!(!should_save_preferences(true, &[Applied::Declined]));
    }

    /// With both Desktop and Start Menu configured, declining either one
    /// blocks the save. Preferences are a single setting shared by every
    /// target, so a partial refusal cannot be honored partially, and refusing
    /// to write is the recoverable direction. Both orders are checked so this
    /// cannot pass by only ever inspecting the first or the last entry.
    #[test]
    fn declining_any_one_target_blocks_the_save_whichever_order_it_came_in() {
        assert!(!should_save_preferences(
            false,
            &[Applied::Proceeded, Applied::Declined]
        ));
        assert!(!should_save_preferences(
            false,
            &[Applied::Declined, Applied::Proceeded]
        ));
        assert!(should_save_preferences(
            false,
            &[Applied::Proceeded, Applied::Proceeded]
        ));
    }

    // --- record_platform / exit codes (finding 1) ---

    /// The central fix for finding 1: a platform that is installed but
    /// unreadable must become a failed *item*, appended to `failures`, and
    /// must NOT be reported as if it were merely absent (that would hide a
    /// real problem) and must NOT abort anything (there is nothing here to
    /// abort — `record_platform` has no way to signal "stop the whole
    /// run", which is the point).
    #[test]
    fn a_provider_read_failure_becomes_a_failed_item_not_an_installed_note() {
        let mut games = Vec::new();
        let mut infos = Vec::new();
        let mut failures = Vec::new();

        record_platform(
            Some(Err("disk is unreadable".to_string())),
            "not installed note (must not appear)",
            "Steam is installed but could not be read",
            &mut games,
            &mut infos,
            &mut failures,
        );

        assert!(games.is_empty());
        assert!(
            infos.is_empty(),
            "a read failure must never be reported as an install note"
        );
        assert_eq!(
            failures,
            vec!["Steam is installed but could not be read: disk is unreadable"]
        );
    }

    /// The other half of the same distinction: a platform that was never
    /// installed must land in `infos`, never in `failures` — it is not a
    /// problem, it is the common case for most users.
    #[test]
    fn a_platform_that_is_not_installed_produces_an_info_note_not_a_failure() {
        let mut games = Vec::new();
        let mut infos = Vec::new();
        let mut failures = Vec::new();

        record_platform(
            None,
            "Epic Games Launcher is not installed; skipping.",
            "unreadable prefix (must not appear)",
            &mut games,
            &mut infos,
            &mut failures,
        );

        assert_eq!(
            infos,
            vec!["Epic Games Launcher is not installed; skipping."]
        );
        assert!(failures.is_empty());
    }

    /// A successful read must append its games and produce neither an info
    /// nor a failure — the ordinary, silent, working case.
    #[test]
    fn a_successful_detection_appends_its_games_without_any_note() {
        let mut games = Vec::new();
        let mut infos = Vec::new();
        let mut failures = Vec::new();

        record_platform(
            Some(Ok(vec![test_game("440", "Team Fortress 2")])),
            "note (must not appear)",
            "prefix (must not appear)",
            &mut games,
            &mut infos,
            &mut failures,
        );

        assert_eq!(games.len(), 1);
        assert!(infos.is_empty());
        assert!(failures.is_empty());
    }

    /// `exit_code` is the single place that turns a failed-item count into
    /// a process exit code. Finding 1 depended on this never reaching `2`
    /// — that code is reserved for `main`'s own `ComGuard::new()` check —
    /// so this pins that down directly: any positive count, however large,
    /// collapses to `1`.
    #[test]
    fn exit_code_collapses_any_positive_failure_count_to_one_and_never_to_two() {
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(1), 1);
        assert_eq!(
            exit_code(2),
            1,
            "two failures must still be exit code 1, not 2"
        );
        assert_eq!(exit_code(42), 1);
    }

    // --- resolve_bases (findings 2 and 3) ---

    /// The mechanism behind both findings 2 and 3: one target failing to
    /// resolve must not hide the other, and the failure must be named
    /// rather than merged into silence. This is exercised with a fake
    /// resolver because the real `layout::target_dir` only fails via a live
    /// Windows known-folder lookup, which cannot be forced from a test.
    #[test]
    fn resolve_bases_keeps_the_resolved_target_and_names_the_unresolved_one() {
        let (resolved, unresolved) =
            resolve_bases([Target::Desktop, Target::StartMenu], |t| match t {
                Target::Desktop => Ok(PathBuf::from(r"C:\Desktop")),
                Target::StartMenu => Err(LayoutError::UnknownFolder("boom".into())),
            });

        assert_eq!(
            resolved,
            vec![(Target::Desktop, PathBuf::from(r"C:\Desktop"))]
        );
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].0, Target::StartMenu);
        assert!(
            unresolved[0].1.contains("boom"),
            "the resolver's own error detail must be preserved, got {:?}",
            unresolved[0].1
        );
    }

    /// When every target resolves, nothing is lost or misreported as
    /// unresolved.
    #[test]
    fn resolve_bases_reports_no_unresolved_targets_when_everything_resolves() {
        let (resolved, unresolved) = resolve_bases([Target::Desktop, Target::StartMenu], |t| {
            Ok(PathBuf::from(r"C:\Base").join(t.label()))
        });
        assert_eq!(resolved.len(), 2);
        assert!(unresolved.is_empty());
    }

    /// `warn_unresolved` is what turns `resolve_bases`'s output into the
    /// failed-item count `sync`, `clean` and `status` all fold into their
    /// exit code - the piece that makes an unresolvable target actually reach
    /// the exit code rather than only the printed warning.
    ///
    /// The previous version of this test passed a one-element slice and
    /// asserted the answer was 1, which `[_].len()` satisfies on its own: a
    /// body that ignored its argument entirely and returned a constant would
    /// have passed. Counting is only observable across more than one entry,
    /// and only if a repeated target is counted twice rather than collapsed -
    /// two targets that both failed are two failed items, not one.
    #[test]
    fn warn_unresolved_counts_every_unresolved_target_including_repeats() {
        assert_eq!(warn_unresolved(&[], "scanned"), 0);

        let one = [(Target::StartMenu, "boom".to_string())];
        assert_eq!(warn_unresolved(&one, "scanned"), 1);

        let three = [
            (Target::Desktop, "no desktop".to_string()),
            (Target::StartMenu, "no start menu".to_string()),
            (Target::StartMenu, "and again".to_string()),
        ];
        assert_eq!(
            warn_unresolved(&three, "scanned"),
            3,
            "each entry is one failed item; a repeated target must not collapse"
        );
    }

    // --- summary_line (correction 3 / dry-run wording) ---

    /// Correction 3's runtime half: after a dry run, the summary line must
    /// say "would be created", never bare "created" — the latter would
    /// contradict the tool's own `--help` promise that a dry run does not
    /// modify the user's shortcuts.
    #[test]
    fn summary_line_uses_conditional_wording_under_dry_run() {
        let s = ops::Summary {
            created: 3,
            updated: 1,
            ..Default::default()
        };

        let dry = summary_line(&s, true);
        assert!(dry.contains("3 would be created"), "got: {dry}");
        assert!(dry.contains("1 would be updated"), "got: {dry}");
        assert!(
            !dry.contains("3 created,"),
            "must not read as if it already happened: {dry}"
        );

        let real = summary_line(&s, false);
        assert!(real.contains("3 created,"), "got: {real}");
        assert!(
            !real.contains("would be"),
            "a real run must not hedge: {real}"
        );
    }

    // --- should_open_after_sync ---

    #[test]
    fn should_open_after_sync_respects_dry_run_declined_and_flags() {
        assert_eq!(
            should_open_after_sync(true, false, true, &[Applied::Proceeded]),
            OpenDecision::Skip
        );
        assert_eq!(
            should_open_after_sync(false, false, true, &[Applied::Declined]),
            OpenDecision::Skip
        );
        assert_eq!(
            should_open_after_sync(false, true, true, &[Applied::Proceeded]),
            OpenDecision::OpenNow
        );
        assert_eq!(
            should_open_after_sync(false, true, false, &[Applied::Proceeded]),
            OpenDecision::Skip
        );
        assert_eq!(
            should_open_after_sync(false, false, false, &[Applied::Proceeded]),
            OpenDecision::Ask
        );
    }
}
