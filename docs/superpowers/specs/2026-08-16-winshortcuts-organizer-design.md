# WinShortcutsOrganizer — Design

**Date:** 2026-08-16
**Status:** Approved for planning
**Language:** Rust (single self-contained `.exe`, published on GitHub)

---

## 1. Problem

Game platforms create Windows shortcuts badly, or not at all.

**Steam** creates `.url` files whose target is a protocol link (`steam://rungameid/<appid>`). A `.url` is a web bookmark, not a real shortcut: it has no stable icon slot, and Windows loses its icon when the file is moved, when the icon cache is rebuilt, or when the `.url` handler association changes. The result is the well-known wall of blank white icons in the Start Menu. Existing tools treat this as an icon bug to patch; it is a consequence of using the wrong file format.

**Epic Games Launcher** has no native Start Menu shortcut support at all. Every shortcut is manual.

Both platforms leave the user doing repetitive manual work: create shortcut, rename, move to `shell:programs`, repeat per game. There is no supported way to undo it in bulk, and no way to re-run it safely after installing new games.

## 2. Goals

- Generate real Windows `.lnk` shortcuts for installed games, with correct icons, for the Desktop and/or the per-user Start Menu.
- Support Steam and Epic at launch, behind an abstraction that makes adding GOG / EA / Xbox a new file rather than a refactor.
- Be safely re-runnable: running twice does not duplicate, and picks up newly installed games.
- Be safely reversible: remove everything the tool created, and nothing else.
- Migrate existing broken Steam `.url` shortcuts to working `.lnk` shortcuts.
- Require no administrator rights, ever.

## 3. Non-goals

- **Pinning tiles to the Start Menu panel.** Microsoft removed programmatic pinning in Windows 10 1607; the `PinToStartScreen` verb is blocked. The tool places `.lnk` files in the Start Menu's Programs folder, which makes games appear under *All apps* and searchable by keyboard. Pinning to the panel remains a manual right-click. This must be stated plainly in the README so the tool does not promise what Windows does not allow.
- Games that are owned but not installed. Detection is install-based and offline; no web API, no login, no Steam API key.
- Being a game launcher or library UI. That is Playnite's job.
- Managing non-game applications. Planned as a future module, out of scope here.

## 4. Key decisions

### 4.1 Generate `.lnk`, never `.url`

A `.lnk` created via `IShellLinkW` has an `IconLocation` field that is **independent of its `Target`**. This is the decision the whole design rests on: the shortcut can launch through Steam while drawing its icon from a completely different file.

- Target: `steam.exe -applaunch <appid>` — the game still starts through Steam, with overlay, cloud saves and playtime tracking intact.
- Icon: a path to the game's own executable. Windows renders it directly; no extraction step.

### 4.2 Icons come from the game's executable

The original plan was to read Steam's icon cache, which requires parsing `appcache/appinfo.vdf` — a proprietary binary format whose version changed from v28 to v29 in 2024. Inspection of a real installation killed that idea on its own merits:

- `Steam/steam/games/` contained **one** `.ico` file for a library of nine installed games. The cache is only populated when Steam itself creates a shortcut, so it cannot be relied on.
- `appcache/librarycache/<appid>/` has hundreds of entries, but they hold `logo.png`, `library_600x900.jpg`, `library_hero.jpg` — no `.ico`.

Pointing `IconLocation` at the game's executable is better on every axis: no network, no binary format parsing, no dependency on Steam's cache state, and the icon is the one the developer shipped. **`appinfo.vdf` is not parsed anywhere in this design.** This removes the project's only significant technical risk.

The cost is that Steam's manifests do not say which executable is the game. See §6.2.

### 4.3 Ownership is recorded in the shortcut itself

The tool must know which shortcuts are its own, so that `clean` never deletes a shortcut the user made by hand, and `sync` never duplicates.

A sidecar state file (`state.json`) was rejected: it desynchronizes the moment the user moves or deletes a `.lnk` manually, after which `clean` leaves orphans or tries to delete files that are gone. Reconciling it against disk would be required anyway.

Instead, each generated `.lnk` carries a marker in its `Description` field, which Windows surfaces as the hover tooltip:

```
Terraria — managed by WinShortcutsOrganizer [wso:steam:105600]
```

Human-readable, machine-parseable, and the filesystem is the single source of truth. Scanning a few hundred `.lnk` files via COM takes milliseconds.

User preferences and per-game exclusions do need to persist, and live in a small `config.toml` under `%APPDATA%\WinShortcutsOrganizer\`. That file holds choices, never ownership.

### 4.4 Never require administrator rights

The tool writes only to `FOLDERID_Desktop` and `FOLDERID_Programs` — the **per-user** Start Menu. The all-users Start Menu (`FOLDERID_CommonPrograms`) is never touched, read or written. This eliminates the entire class of elevation failures by construction.

## 5. Architecture

Decision logic is pure and side-effect free. Everything dangerous — COM, registry, filesystem — lives at the edges behind narrow interfaces. Roughly 80% of the program is testable without Windows, Steam or Epic installed.

```
src/
  main.rs            entry point, dispatch
  cli.rs             clap command and flag definitions
  wizard.rs          interactive prompts (inquire)
  config.rs          config.toml: preferences + exclusions
  platform/
    mod.rs           GameProvider trait, Game / Launch types
    steam.rs         library discovery, .acf parsing, VDF reader
    epic.rs          .item manifest parsing
  shortcut/
    mod.rs           Shortcut model
    writer.rs        IShellLinkW + IPersistFile create / read
    signature.rs     [wso:platform:id] encode / decode
    url_legacy.rs    .url parsing, for migrate
  icon/
    mod.rs           candidate chain resolution
    exe_finder.rs    executable scoring heuristic
    pe.rs            does this .exe contain an icon resource?
    convert.rs       PNG -> ICO fallback
  layout.rs          target directories, folder modes, name sanitization
  plan.rs            pure reconciliation
  ops.rs             apply actions, collect per-item results
```

### 5.1 The pure core

```rust
fn plan(desired: &[DesiredShortcut], owned: &[OwnedShortcut]) -> Vec<Action>

enum Action {
    Create(DesiredShortcut),
    Retarget { path: PathBuf, to: Launch },
    FixIcon  { path: PathBuf, to: IconSource },
    Move     { from: PathBuf, to: PathBuf },   // folder mode changed
    DeleteOrphan(PathBuf),                     // game uninstalled or excluded
    Skip { name: String, reason: SkipReason },
}
```

`desired` is derived from providers plus preferences. `owned` is derived from scanning the target directories and keeping only signed shortcuts. Every command is this one function with different inputs:

| Command | `desired` | `owned` | Applies actions |
|---|---|---|---|
| `sync` | all selected games | scanned | yes |
| `status` | all selected games | scanned | no, prints only |
| `clean` | empty | scanned | yes (deletes only) |

One code path to reason about and to test.

### 5.2 Provider abstraction

```rust
trait GameProvider {
    fn platform(&self) -> Platform;
    fn detect() -> Option<Self> where Self: Sized;   // None if not installed
    fn games(&self) -> Result<Vec<Game>>;
}

struct Game {
    id: GameId,                    // Platform + native id
    name: String,                  // display name, unsanitized
    install_dir: PathBuf,
    launch: Launch,
    icon_candidates: Vec<IconCandidate>,   // ordered, first viable wins
}

struct Launch { program: PathBuf, args: String, working_dir: Option<PathBuf> }
```

A single `Launch` shape covers both platforms; the difference is in what each provider puts there (§6.1, §7.1). The icon resolver that walks `icon_candidates` and validates each one is shared, so providers only declare where their icons might come from. Adding GOG means writing one file that implements the trait.

## 6. Steam provider

### 6.1 Discovery and launch

Steam's install directory is resolved in order: `HKCU\Software\Valve\Steam\SteamPath`, then `HKLM\SOFTWARE\WOW6432Node\Valve\Steam\InstallPath`, then the conventional paths (`C:\Program Files (x86)\Steam`). The fallback chain is not optional — during validation on a real machine the `HKCU` query returned nothing while Steam was correctly installed at the conventional path.

`steamapps/libraryfolders.vdf` lists every library root (text VDF). Each library's `steamapps/appmanifest_<appid>.acf` yields `appid`, `name` and `installdir`.

Launch target: `<steam>/steam.exe` with args `-applaunch <appid>`.

Non-games must be filtered. `.acf` files do not carry a type field, so filtering uses a blocklist of known infrastructure appids — `228980` (Steamworks Common Redistributables), Proton and Steam Linux Runtime entries — plus the user's own exclusions. This blocklist is data, kept in one place, and easy to extend.

### 6.2 Executable identification

`installdir` gives the folder; nothing gives the executable. The heuristic, in order:

1. Collect `.exe` files up to depth 4 under the install directory.
2. Drop blocklist matches: `*CrashHandler*`, `*CrashReport*`, `*Setup*`, `*Installer*`, `dotNetFx*`, `vcredist*`, `DXSETUP*`, anything under `_CommonRedist/`.
3. **Prefer the shallowest depth that still has candidates.** Depth dominates.
4. Within that depth, prefer the best normalized-name match against the game name and `installdir`.
5. Break remaining ties by file size.

Depth outranking size is the load-bearing part, and came from real data. Verified by inspection against a nine-game library:

| Game | Correct executable | What breaks the naive rule |
|---|---|---|
| Terraria | `Terraria.exe` | `TerrariaServer.exe` also name-matches; size decides |
| Persona 3 Reload | `P3R/Binaries/Win64/P3R.exe` | no root-level exe at all; UE layout |
| Monster Hunter Wilds | `MonsterHunterWilds.exe` | `CrashReport.exe` is 155 MB |
| Overwatch | `Overwatch.exe` | — |
| X4: Foundations | `X4.exe` | name is only a partial match |
| **Garry's Mod** | `gmod.exe` (430 KB, depth 1) | **largest is `bin/win64/studiomdl.exe` at 2.8 MB, depth 3** |
| Helldivers 2 | `bin/helldivers2.exe` | `tools/GGSetup.exe` is a close second by size |
| **Supermarket Together** | `Supermarket Together.exe` (668 KB) | **`UnityCrashHandler64.exe` is larger at 992 KB** |

Size alone fails on 2 of 8. Depth + blocklist + name resolves all 8.

A `[icons]` table in `config.toml` lets the user override the resolved icon per game, for the cases the heuristic gets wrong.

## 7. Epic provider

### 7.1 Discovery and launch

Epic writes one JSON manifest per installed item to `C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests\*.item`, containing `DisplayName`, `InstallLocation`, `LaunchExecutable`, `AppName`, `CatalogNamespace`, `CatalogItemId` and `AppCategories`. Plain JSON — no custom parser needed.

Items are kept only when `AppCategories` contains `games`, which excludes DLC, plugins and Unreal Engine installs. Items flagged as incomplete installs are skipped with a reason.

A `.lnk` cannot target a protocol URI. Epic launches through Explorer:

- program: `explorer.exe`
- args: `com.epicgames.launcher://apps/<Namespace>%3A<CatalogItemId>%3A<AppName>?action=launch&silent=true`

Launching through the URI rather than the executable directly preserves Epic's own launch handling and entitlement checks.

### 7.2 Icons

Epic declares `LaunchExecutable` explicitly, so `InstallLocation/LaunchExecutable` is an exact icon source. No heuristic is needed on this platform.

## 8. Icon resolution

The shared resolver walks each game's ordered candidates and takes the first that validates. Validation means: the file exists, and for an `.exe` it actually contains an icon resource (`RT_GROUP_ICON`) — checked by reading the PE resource directory. An executable with no icon falls through rather than producing a generic blank.

**Steam:** game executable → `librarycache/<appid>/logo.png` converted to `.ico` and written to `%LOCALAPPDATA%\WinShortcutsOrganizer\icons\<appid>.ico` → `steam.exe`.

**Epic:** declared executable → `EpicGamesLauncher.exe`.

**Migration** has a better source than either: an existing Steam `.url` already contains the `IconFile` line that Steam itself resolved. When migrating, that path is used directly when the file still exists, ahead of the heuristic.

Icon paths are recorded as absolute paths in the `.lnk`. If a game is moved to another library, the icon breaks until the next `sync`, which detects and repairs it via `FixIcon`.

## 9. Layout and naming

**Targets:** Desktop, Start Menu, or both.

**Organization modes:**
- `flat` — shortcuts directly in the target directory
- `unified` — one folder for all platforms, name configurable, default `Games`
- `per-platform` — one folder per platform (`Steam`, `Epic`), inside an optional parent

Switching modes on a later run produces `Move` actions rather than duplicates.

**Name sanitization** strips characters illegal in NTFS filenames — `\ / : * ? " < > |` — and trims trailing dots and spaces. This is not theoretical: `X4: Foundations` contains a colon. Trademark symbols (`®`, `™`) are legal and preserved, so `Overwatch®` and `HELLDIVERS™ 2` keep their names. Reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`) are suffixed.

**Collisions:** if the same title exists on two platforms, or two sanitized names collide, the platform name is appended — `Fallout 4 (Epic)`.

## 10. CLI surface

`wso.exe` with no arguments runs `sync` interactively, so double-clicking the executable works.

| Command | Behavior |
|---|---|
| `sync` | Detect platforms and games, prompt for target / organization / folder name, show a summary of pending actions, confirm, apply. Reconciles on re-run. |
| `migrate` | Find Steam `.url` shortcuts on Desktop and Start Menu, convert to `.lnk` with correct icons, remove the originals after successful conversion. |
| `clean` | Delete only signed shortcuts. |
| `status` | Report what exists, where, and what is out of sync. No side effects. |

Flags cover the same ground for automation and are never required: `--platforms steam,epic`, `--target desktop|start-menu|both`, `--organize flat|unified|per-platform`, `--folder <name>`, `--dry-run`, `--yes`.

Every destructive operation prints its plan and requires confirmation unless `--yes` is passed. `sync` never deletes an unsigned file under any circumstances.

All program output, `--help` text and the README are in English.

## 11. Error handling

A failure on one game never aborts the run. Each game is processed into its own `Result`, and the run ends with a summary: created, updated, skipped (with reason), failed (with reason).

- A platform that is not installed is an informational line, not an error.
- A game whose executable cannot be identified still gets a shortcut, using the fallback icon, and is reported as such.
- A locked or read-only target file is reported per item and the run continues.

Exit code is `0` when every item succeeded, `1` when any item failed, `2` on a global failure such as an unreadable Steam installation.

## 12. Testing strategy

**Pure unit tests, no Windows APIs required:**
- `plan()` — every action variant, plus idempotency (running against its own output produces only `Skip`).
- Executable scoring — the eight-game table in §6.2 becomes fixtures of synthetic file trees. Garry's Mod and Supermarket Together are explicit regression cases, since they are what a naive size-based implementation gets wrong.
- Name sanitization — colon, reserved names, trademark symbols, collisions.
- Signature encode / decode, including malformed and foreign descriptions.
- Text VDF parsing against real `libraryfolders.vdf` and `.acf` samples.
- Epic `.item` parsing, including a DLC item that must be filtered out.
- Path construction for all three organization modes.

**Integration tests (`#[cfg(windows)]`):**
- Write a `.lnk` via COM into a temp directory, read it back, assert target, args, icon and description round-trip.
- Scan a temp directory containing signed shortcuts, unsigned shortcuts and unrelated files; assert only signed ones are claimed.

**Manual verification:** `--dry-run` against a real installation before any release.

## 13. Dependencies

`clap`, `inquire`, `serde` + `serde_json`, `toml`, `windows`, `image` + `ico`, `thiserror`.

Steam's text VDF is parsed by a small hand-written parser rather than a crate. The format is simple, its exact shape is known from validation, and it is the only format needed — not worth a dependency of uncertain maintenance.

## 14. Risks

| Risk | Mitigation |
|---|---|
| Executable heuristic picks the wrong file for an untested game | Fallback chain still produces a usable icon; `[icons]` override in `config.toml`; regression fixtures grow as cases are reported |
| Epic changes its manifest schema or launch URI | JSON parsing is tolerant of unknown fields; missing required fields skip the item with a clear reason rather than crashing |
| Steam changes `.acf` or `libraryfolders.vdf` layout | Both are stable text formats; parser failures degrade to "platform unreadable" with a diagnostic, not a panic |
| Icon paths break when a game moves libraries | `sync` detects and repairs via `FixIcon` |
| Users expect automatic pinning to the Start panel | Stated as a non-goal in the README and in the wizard's completion message |
| Unsigned `.exe` triggers SmartScreen on download | Documented in the README; signing is a later concern |

## 15. Future modules

The provider trait and the pure planner are the extension points. Anticipated, explicitly not built now:

- Additional providers: GOG Galaxy, EA App, Ubisoft Connect, Xbox / Microsoft Store, Battle.net.
- A general application organizer: group existing Start Menu entries into folders by user-defined rules.
- Custom entries: add non-platform games or programs to the same managed layout.

Each is additive. None requires changing `plan()`.
