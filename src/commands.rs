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
use crate::layout::{self, LayoutError, Organization, Target};
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

    Detection { games, infos, failures }
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
) -> (Vec<(Target, PathBuf)>, Vec<(Target, String)>)
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
fn warn_unresolved(unresolved: &[(Target, String)], action: &str) -> usize {
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

fn confirm(prompt: &str, assume_yes: bool) -> bool {
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
fn exit_code(failed: usize) -> i32 {
    if failed > 0 {
        1
    } else {
        0
    }
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
/// target under the configured organization. `base` is already resolved by
/// the caller (see `resolve_bases`), so this never fails.
fn sync_target(
    cfg: &config::Config,
    games: &[Game],
    base: &Path,
    target: Target,
    dry_run: bool,
    assume_yes: bool,
) -> ops::Summary {
    let cache = config::cache_dir();

    let owned = ops::scan_owned(&ops::managed_dirs(base, &cfg.organization));
    let desired = ops::build_desired(games, cfg, base, &cache);
    let actions = plan(&desired, &owned);

    apply_with_confirmation(target.label(), &actions, dry_run, assume_yes)
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

    let (games, platform_failures) = report_detection(collect_games(&cfg));
    let mut failed = platform_failures;

    let (resolved, unresolved) = resolve_bases(cfg.targets.iter().copied(), layout::target_dir);
    failed += warn_unresolved(&unresolved, "synced");

    for (target, base) in resolved {
        let s = sync_target(&cfg, &games, &base, target, cli.dry_run, cli.yes);
        print_summary(&s, cli.dry_run);
        failed += s.failed;
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
/// are still de-duplicated defensively so `scan_owned` never reads the same
/// directory twice. A base that cannot be resolved is returned separately
/// rather than silently dropped — `clean` must tell the user it did not
/// scan everywhere it promised to, not print a clean summary that quietly
/// covered only part of the disk.
fn clean_scan_dirs(cfg: &config::Config) -> (Vec<PathBuf>, Vec<(Target, String)>) {
    let (resolved, unresolved) =
        resolve_bases([Target::Desktop, Target::StartMenu], layout::target_dir);

    let mut dirs = Vec::new();
    for (_, base) in &resolved {
        for org in candidate_organizations(cfg) {
            for d in layout::managed_dirs(base, &org) {
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
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

    let path = config::config_path();
    let cfg = load_config_with_warning(&path);

    let (dirs, unresolved) = clean_scan_dirs(&cfg);
    let mut failed = warn_unresolved(&unresolved, "scanned");

    let owned = ops::scan_owned(&dirs);
    let actions = plan(&[], &owned);

    let summary = apply_with_confirmation("Every managed layout", &actions, cli.dry_run, cli.yes);
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
    exit_code(failed)
}

pub fn migrate(_cli: &Cli) -> i32 {
    println!("migrate is implemented in Task 15");
    0
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

    // --- candidate_organizations (correction 1) ---

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
        assert!(infos.is_empty(), "a read failure must never be reported as an install note");
        assert_eq!(failures, vec!["Steam is installed but could not be read: disk is unreadable"]);
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

        assert_eq!(infos, vec!["Epic Games Launcher is not installed; skipping."]);
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
        assert_eq!(exit_code(2), 1, "two failures must still be exit code 1, not 2");
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
        let (resolved, unresolved) = resolve_bases(
            [Target::Desktop, Target::StartMenu],
            |t| match t {
                Target::Desktop => Ok(PathBuf::from(r"C:\Desktop")),
                Target::StartMenu => Err(LayoutError::UnknownFolder("boom".into())),
            },
        );

        assert_eq!(resolved, vec![(Target::Desktop, PathBuf::from(r"C:\Desktop"))]);
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
        let (resolved, unresolved) =
            resolve_bases([Target::Desktop, Target::StartMenu], |t| {
                Ok(PathBuf::from(r"C:\Base").join(t.label()))
            });
        assert_eq!(resolved.len(), 2);
        assert!(unresolved.is_empty());
    }

    /// `warn_unresolved` is what turns `resolve_bases`'s output into the
    /// failed-item count `sync`, `clean` and `status` all fold into their
    /// exit code — this is the piece that makes finding 2/3's fix ("name it
    /// and exit 1") actually reach the exit code rather than only the
    /// printed warning.
    #[test]
    fn warn_unresolved_counts_each_unresolved_target_exactly_once() {
        let unresolved =
            vec![(Target::StartMenu, "could not locate a Windows known folder: boom".to_string())];
        assert_eq!(warn_unresolved(&unresolved, "scanned"), 1);
        assert_eq!(warn_unresolved(&[], "scanned"), 0);
    }

    // --- summary_line (correction 3 / dry-run wording) ---

    /// Correction 3's runtime half: after a dry run, the summary line must
    /// say "would be created", never bare "created" — the latter would
    /// contradict the tool's own `--help` promise that a dry run does not
    /// modify the user's shortcuts.
    #[test]
    fn summary_line_uses_conditional_wording_under_dry_run() {
        let s = ops::Summary { created: 3, updated: 1, ..Default::default() };

        let dry = summary_line(&s, true);
        assert!(dry.contains("3 would be created"), "got: {dry}");
        assert!(dry.contains("1 would be updated"), "got: {dry}");
        assert!(!dry.contains("3 created,"), "must not read as if it already happened: {dry}");

        let real = summary_line(&s, false);
        assert!(real.contains("3 created,"), "got: {real}");
        assert!(!real.contains("would be"), "a real run must not hedge: {real}");
    }
}
