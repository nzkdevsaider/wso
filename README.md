<div align="center">

# WinShortcutsOrganizer (`wso`)

**Create real Windows `.lnk` shortcuts with working icons for your Steam and Epic Games libraries.**

[![CI](https://github.com/nzkdevsaider/wso/actions/workflows/ci.yml/badge.svg)](https://github.com/nzkdevsaider/wso/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/nzkdevsaider/wso)](https://github.com/nzkdevsaider/wso/releases)
[![Platform](https://img.shields.io/badge/platform-Windows-0078D6?logo=windows&logoColor=white)](#building-from-source)
[![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust&logoColor=white)](#building-from-source)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)

</div>

---

Steam `.url` web bookmarks often lose their icons, and the Epic Games Launcher doesn't always organize shortcuts where you want them. `wso` scans your installed games across all drives and creates real Windows `.lnk` shortcuts on your **Desktop** and/or **Start Menu** — with icons resolved directly from game executables or storefront artwork.

> [!NOTE]
> **No administrator rights required.** `wso` only writes to your own Desktop and per-user Start Menu Programs folder — locations a standard user account already owns.

---

## Features

- **Multi-storefront detection** — Supports **Steam** (across all `libraryfolders.vdf` drives) and **Epic Games Launcher** (both the classic client and the new **Preview / `eglv2`** client, across any drive).
- **Real `.lnk` shortcuts with working icons** — Resolves icons from the game's executable, falls back to storefront library artwork (converted to multi-resolution `.ico` in `%LOCALAPPDATA%`), or uses the launcher icon as a last resort.
- **Flexible organization** — Place shortcuts directly (`flat`), in a single folder (`unified`), or grouped into `Steam` / `Epic` subfolders (`per-platform`).
- **Idempotent & non-destructive** — Safe to re-run at any time. Only shortcuts created by `wso` are ever updated, moved, or deleted; your own shortcuts are never touched.
- **Interactive & scriptable** — Run with no arguments for an interactive setup wizard, or pass CLI flags for unattended runs.

---

## Install

Download `wso.exe` from the [releases page](https://github.com/nzkdevsaider/wso/releases) and run it. No installer, no administrator rights, and no separate runtime required.

---

## Usage

Run `wso.exe` with no arguments to launch the interactive setup wizard:

```powershell
wso.exe
```

Or invoke a subcommand directly:

| Command       | Description                                                        |
| ------------- | ------------------------------------------------------------------ |
| `wso sync`    | Create, update, and organize shortcuts for installed games.        |
| `wso migrate` | Convert Steam's legacy `.url` shortcuts into real `.lnk` files.    |
| `wso clean`   | Remove every shortcut `wso` created — nothing else.                |
| `wso status`  | Scan your libraries and report what exists and what is out of sync.|

> [!TIP]
> Add `--dry-run` to `sync`, `migrate`, or `clean` to preview pending changes without modifying your Desktop or Start Menu.
> Note that `sync --dry-run` and `status` still resolve icons while building the plan, which may populate the internal icon cache in `%LOCALAPPDATA%`; `migrate --dry-run` and `clean` never touch the cache.

### Flags

```text
--target <TARGET>       desktop | start-menu | both
--organize <MODE>       flat | unified | per-platform
--folder <NAME>         folder name for unified or per-platform mode
--platforms <LIST>      comma-separated: steam,epic
--dry-run               preview changes without modifying shortcuts
--yes                   skip confirmation prompts
```

Passing any of `--target`, `--organize`, or `--yes` skips the interactive wizard and runs non-interactively, making `wso` easy to use in scripts and scheduled tasks.

**Example:**

```powershell
wso sync --target desktop --organize per-platform --folder "Games" --yes
```

### Exit Codes

| Code | Meaning                                                                                                                                      |
| ---- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `0`  | Every item succeeded.                                                                                                                        |
| `1`  | Partial failure (e.g., an unreadable platform, unresolvable folder, or unwritable shortcut) — the run still processed everything else it could.|
| `2`  | Fatal startup failure before any command ran (Windows COM failed to initialize).                                                             |

---

## Safety & Ownership

`wso` embeds an ownership marker in the **Description** field of every shortcut it creates (e.g. `[wso:steam:440]`, visible in the shortcut's hover tooltip):

- `sync` never overwrites or removes a shortcut that does not carry its marker.
- `clean` deletes only shortcuts carrying the `[wso:...]` marker, across all layouts (`flat`, `unified`, `per-platform`) on both Desktop and Start Menu.
- Any shortcut you created manually or moved into a managed folder is recognized as foreign and left untouched.

---

## Configuration

Preferences are saved in `%APPDATA%\WinShortcutsOrganizer\config.toml`. The wizard manages this file, and you can also edit it by hand to exclude games or override icons:

```toml
exclusions = ["steam:228980", "Wallpaper Engine"]

[icon_overrides]
"steam:440" = "C:\\icons\\tf2.ico"
```

- **Exclusions**: Use a qualified `platform:id` (e.g. `steam:228980`) to match one game on one platform, or a bare, case-insensitive title to match across platforms.
- **Icon overrides**: Keyed by `platform:id` and pointing to a local `.ico` or executable.
- **Corrupt config protection**: If `config.toml` cannot be parsed, `wso` falls back to defaults instead of crashing and preserves your broken file as `config.toml.invalid` before writing any new preferences.

---

## Building from Source

Requires a Rust toolchain on **Windows** (shortcut and icon handling uses the Windows Shell COM APIs):

```powershell
cargo build --release
```

Run the checks used in CI:

```powershell
cargo test --all
cargo clippy -- -D warnings
cargo fmt --check
```

`wso --version` stamps the binary version at build time: commits tagged `v<version>` report the bare version (`wso 0.2.6`), while other commits append the short git SHA (`wso 0.2.6-dev.a1b2c3d`). Built outside a git checkout, the bare `Cargo.toml` version is reported.

---

## License

[MIT](LICENSE)
