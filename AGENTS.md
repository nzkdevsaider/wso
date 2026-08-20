# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## What this is

`wso` (crate `winshortcutsorganizer`, binary `wso`) is a Windows-only Rust CLI that writes real
`.lnk` shortcuts with working icons for games installed through Steam and the Epic Games Launcher.
Commands: `sync`, `migrate`, `clean`, `status` (`sync` is the default when no subcommand is given).

## Commands

```
cargo build                       # debug
cargo build --release
cargo test --all                  # what CI runs
cargo clippy -- -D warnings       # CI gate: warnings are errors
cargo fmt --check                 # CI gate
```

Single tests:

```
cargo test --test idempotence                 # one integration test file (tests/*.rs)
cargo test --lib plan::tests                  # one unit-test module
cargo test create_refuses_to_overwrite     # one test by name substring
cargo test -- --nocapture                     # see println! from tests
```

Running it non-interactively (the wizard needs a real terminal, so always pass flags when driving
it yourself):

```
cargo run -- sync --dry-run --yes --target desktop --organize flat
cargo run -- status
```

CI (`.github/workflows/ci.yml`) runs on `windows-latest` only, and deliberately: the shortcut and
icon code goes through Windows Shell COM, and five integration test files are `#![cfg(windows)]`.
A Linux run compiles the stubbed non-Windows paths and skips most of what matters.

## Branches and releases

`development` is where work lands; `master` holds what has been released. Nothing is a release
until it carries a tag.

- Push to any branch or open a PR: `ci.yml` runs the three gates. No binary.
- Push to `development`: `release.yml` runs the gates, then uploads `wso.exe` as a build artifact
  named `wso-dev-{sha}`, kept 14 days. Reachable only from the Actions run.
- Push a `v*` tag: `release.yml` runs the gates, then publishes a GitHub Release with `wso.exe`
  attached. Permanent and public.

`ci.yml` ignores `development` and every tag (`branches-ignore` / `tags-ignore`) because
`release.yml` invokes it through `workflow_call`; without that the suite would run twice.

Cutting a release means bumping `version` in `Cargo.toml`, merging to `master`, then tagging that
commit `v{version}`. `release.yml` refuses to publish when the tag and the version the binary
reports disagree, which is what a tag pushed before the bump looks like.

## Architecture

One pipeline, run per target directory (Desktop, Start Menu):

```
providers          -> Vec<Game>            platform::{steam,epic}, via GameProvider
ops::build_desired -> Vec<DesiredShortcut> games + config + layout + resolved icons
ops::scan_owned    -> Vec<OwnedShortcut>   .lnk files carrying our signature
plan::plan         -> Vec<Action>          pure diff: Create/Retarget/FixIcon/Move/DeleteOrphan/Skip
ops::apply         -> Vec<Outcome>         the only code that mutates the filesystem
```

`src/commands.rs` is the orchestration/presentation layer that wires those together, prints, and
asks for confirmation; `src/main.rs` only initializes COM and dispatches.

The layering is the point and should be preserved:

- **Pure** — `plan.rs`, `layout.rs`, `shortcut/signature.rs`, `shortcut/collision.rs`, `config.rs`
  (parsing/merging), `paths.rs`. No I/O, so every command's decision-making is testable without
  Windows, Steam or Epic installed. When a behavior in `commands.rs` needs a test, the fix is to
  extract the decision into a pure function (see `record_platform`, `resolve_bases`,
  `should_save_preferences`, `summary_line`) rather than to capture stdout.
- **I/O** — `ops.rs` (scan/apply), `shortcut/writer.rs` (COM `IShellLink`), `icon/*`,
  `platform/*` (registry, VDF, `.item` manifests), `migrate.rs`, `wizard.rs`.

Windows-specific code is `#[cfg(windows)]` with a `#[cfg(not(windows))]` stub beside it that
returns an error or an empty list, so the crate still compiles and the pure tests still run
elsewhere. Keep that pairing when adding platform code.

`build.rs` stamps the version clap reports as `WSO_VERSION`, read by `env!` in `cli.rs`: a commit
tagged `v{Cargo.toml version}` builds as that version bare, any other commit as
`{version}-dev.{short sha}`. It degrades to the bare version rather than failing when git is
absent, and `tests/version_flag.rs` asserts the binary reports one shape or the other.

## Invariants that are load-bearing

These were each fixed after a real defect; breaking one is a correctness regression, not a style
change. `docs/DECISIONS.md` records the reasoning behind them, and
`docs/superpowers/specs/2026-08-16-winshortcutsorganizer-design.md` is the authority on intent.

- **Ownership marker.** Every shortcut this tool writes carries `[wso:{platform}:{id}]` in its
  Description (`shortcut::signature::encode`/`decode`). `scan_owned` claims only marked files, so
  `plan()` never sees a foreign shortcut and `apply` can never delete one. `Create` and `Move` name
  a *destination*, which a foreign file may already occupy — hence the `shortcut::collision` check
  before both. A shortcut whose marker cannot be read back after writing is refused.
- **Description limit.** A shell link description survives at most `DESCRIPTION_LIMIT_UTF16` (260)
  UTF-16 units. The title is truncated (on whole characters, counted in `len_utf16`) so the marker
  always survives.
- **One extension rule.** `paths::has_extension` is the only way to match a file extension; Windows
  filenames are case-insensitive and four call sites once disagreed about that.
- **Path comparison.** `plan::same_path` folds separators and ASCII case when comparing a shortcut's
  *target* and *working directory* (Windows rewrites those to on-disk casing on save, so raw
  comparison produced endless `Retarget` churn). Shortcut *file* paths are compared exactly, because
  they decide `Move` and `DeleteOrphan` — with the one exception in `ops::names_the_same_file`, which
  keeps a case-only `Move` from deleting the file it just wrote.
- **Partial failure is exit 1.** `0` = everything succeeded, `1` = some item failed (unreadable
  platform, unresolvable known folder, unwritable shortcut) but the run continued, `2` = could not
  start at all (only `ComGuard::new()` failing in `main`). Nothing in `commands.rs` returns `2`, and
  no single bad item may abort a whole run.
- **`clean` scans the disk, not the config.** `ops::scan_dirs` derives candidate directories from
  what is on disk so shortcuts left by a *previous* organization mode are still found.
- **`layout::all_platforms` uses an exhaustive `match`** so adding a `Platform` variant fails to
  compile until every guarded site is updated.
- **Config safety.** A corrupt config falls back to defaults, warns, and is preserved as
  `config.toml.invalid` rather than overwritten; `save` is atomic; a declined confirmation or a
  `--dry-run` must never write preferences back.
- **Console encoding.** `console::use_utf8_output` runs first in `main`, before `Cli::parse` can
  print `--help`: without it a Windows console decodes this program's UTF-8 with the OEM code page
  and every block character and non-ASCII game title comes out as mojibake. The banner's third line
  is indented with `U+00A0`, not a space, because clap trims plain leading whitespace out of
  `before_help`; `banner.rs` and `tests/banner_in_help.rs` both guard that.
- **Icon cache timing.** Icons resolve while the report is *built*, so `sync --dry-run` and `status`
  can still write into `%LOCALAPPDATA%` even though they never touch the Desktop or Start Menu.
  `migrate --dry-run` and `clean` never do. The README documents this precisely; keep them in sync.

## Testing conventions

- `tests/common/mod.rs::unique_temp_dir` names scratch directories by pid + atomic counter; use it
  rather than a fixed temp path, since tests run in parallel.
- Integration tests that touch real `.lnk` files start with `#![cfg(windows)]`.
- The interactive wizard and the "answer no" confirmation path are covered only by unit tests of
  their pure logic — no automated test drives an `inquire` prompt. Changes there are unverified by
  CI and the README says so.

## Style

Rationale lives in `///` doc comments and in `docs/DECISIONS.md`, not in commit-message-only form —
several of the comments in `plan.rs` and `ops.rs` describe the exact defect the code prevents, so
prefer updating them over deleting them when the surrounding logic changes.
