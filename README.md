# WinShortcutsOrganizer

Creates real Windows shortcuts, with correct icons, for the games you already
have installed through Steam and the Epic Games Launcher.

## Why `.lnk` and not `.url`

Steam does not create Start Menu shortcuts for its games at all, and the
Desktop shortcuts it does create are `.url` files — web bookmarks that open a
`steam://` link, not real shortcuts. A `.url` can carry an `IconFile=` line
pointing at an icon Steam *intends* to have cached, but that file is not
always there.

This was verified directly on the development machine: all eight of the
maintainer's Steam Desktop shortcuts are `.url` files, and every one of them
declares an `IconFile=` pointing at a hash-named `.ico` under
`Steam\steam\games\`. None of those eight files exist on disk. Steam's own
icon cache directory held exactly one icon, and it matched none of the eight
`IconFile=` references. That gap — a shortcut confidently pointing at an icon
that was never downloaded — is the actual mechanism behind the blank white
squares Steam shortcuts show in the Start Menu and on the Desktop. It is the
whole reason this project exists.

`wso` writes real `.lnk` shortcuts instead. A `.lnk` keeps its icon reference
independent of the target it launches, so a shortcut can start a game through
Steam or Epic while displaying an icon resolved from the game's own
executable — not from a cache Valve may or may not have populated.

## What it does not do

**It cannot pin anything to the Start menu panel or the taskbar.** This is
the single most important limitation in this document. Windows removed
programmatic Start Menu pinning in Windows 10 version 1607, and no
third-party tool — this one included — can perform it. `wso` places
shortcuts in the Start Menu's Programs folder, where Windows shows them under
*All apps* and finds them when you type their name. Getting a shortcut onto
the pinned Start panel or the taskbar is still a manual right-click from
there, done by the user, every time. If you expected `wso` to pin things for
you, it will not, and that is not a bug report — Microsoft closed that door
in 2016.

**Epic support is less proven than Steam support.** The Epic Games Launcher
is not installed on the development machine, so the Epic provider has only
ever been exercised against synthetic manifest files modeled on the real
`.item` format Epic is documented to write — never against a real installed
launcher or a real game library. Steam, by contrast, has been verified
repeatedly against a real library of installed games. If something about
Epic detection does not work as expected, that is the more likely place to
look first.

**The icon-cache exception is not the same for every command.** Resolving an
icon (probing an executable, converting it, writing the result under
`%LOCALAPPDATA%`) happens as part of building the report of what to do, not
as part of applying it — so whether `--dry-run` also skips the cache write
depends on the command:

- `wso sync --dry-run` and `wso status` (which has no `--dry-run` flag at
  all — reporting *is* everything it does) both resolve icons while building
  their report, so either can write a new file into the icon cache even
  though neither ever touches your Desktop or Start Menu.
- `wso migrate --dry-run` is fully inert: it prints what it would convert and
  returns before resolving any icon, so it does not touch the cache either.
- `wso clean` never resolves an icon in the first place — it only deletes
  shortcuts it recognizes — so the cache is never involved, `--dry-run` or
  not.

In short: no command ever writes to your Desktop or Start Menu without your
having applied the change for real, but `sync --dry-run` and `status` are the
two that can still leave a new file in the icon cache.

**The binary is unsigned.** Windows SmartScreen will show a blue "Windows
protected your PC" warning the first time you run a downloaded `wso.exe`.
Click **More info**, then **Run anyway**. This is expected for any
unsigned executable downloaded from the internet and is not a sign of a
compromised build — see the release page for the exact commit it was built
from.

## What it does do

**No administrator rights are ever required.** `wso` only ever writes to your
own Desktop and to the per-user Start Menu Programs folder — locations a
normal user account already owns. If it ever asks you to run it elevated,
that is a bug, not a requirement.

## Install

Download `wso.exe` from the [releases page](../../releases) and run it. No
installer, no administrator rights, no runtime to install separately.

## Use

Run `wso.exe` with no arguments for an interactive setup wizard that asks
where to place shortcuts, how to organize them, and which platforms to
include.

| Command | What it does |
|---|---|
| `wso sync` | Create and update shortcuts for installed games. Safe to re-run. |
| `wso migrate` | Convert Steam's broken `.url` shortcuts to real `.lnk` files. |
| `wso clean` | Remove every shortcut `wso` created — nothing else. |
| `wso status` | Read your library and report what exists and what is out of sync. |

`status` writes nothing to your Desktop or Start Menu, but — like
`sync --dry-run` — it may still populate the icon cache; see the note above.

Add `--dry-run` to `sync`, `migrate`, or `clean` to preview the actions that
command would take without applying any of them to your Desktop or Start
Menu (the icon cache is a partial exception — see above for exactly which
commands touch it).

### Flags

    --target      desktop | start-menu | both
    --organize    flat | unified | per-platform
    --folder      folder name for unified or per-platform mode
    --platforms   steam,epic
    --dry-run     show changes without applying them (icon cache: see above)
    --yes         skip confirmation prompts

Passing any of `--target`, `--organize`, or `--yes` skips the interactive
wizard and runs non-interactively, which is what the flags exist for in
scripts and CI.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Every item succeeded. |
| `1` | At least one item failed (an unreadable platform, a target folder that could not be resolved, a shortcut that could not be written) — the run still processed everything else it could. |
| `2` | The tool could not start at all, before any command logic ran (Windows COM failed to initialize). |

A partial failure is always `1`, never `2` — `wso` does not abandon a whole
run over one bad item, so scripts should treat `1` as "check the output for
what failed," not as a total failure.

## Safety

`wso` writes a small marker into the Description field of every shortcut it
creates (`[wso:steam:440]`, for example — visible as the shortcut's hover
tooltip). `clean` deletes only shortcuts carrying that marker, and `sync`
never modifies or removes a file it did not create. A shortcut you made by
hand, or moved into a folder `wso` manages, is recognized as foreign and left
alone by both commands.

## Configuration

Preferences persist in `%APPDATA%\WinShortcutsOrganizer\config.toml`. The
wizard writes this file, and it can also be hand-edited. You can exclude
specific games and override the icon `wso` picks for one:

```toml
exclusions = ["steam:228980", "Wallpaper Engine"]

[icon_overrides]
"steam:440" = "C:\\icons\\tf2.ico"
```

An exclusion entry is either a qualified `platform:id` (matches one game on
one platform only) or a bare, case-insensitive game name (matches that name
on any platform). Icon overrides are keyed by the same qualified id.

If the config file becomes unparsable (for example, from a hand-editing
mistake), `wso` falls back to defaults rather than crashing, and preserves
the broken file alongside the new one as `config.toml.invalid` so nothing you
wrote is lost.

## Building from source

    cargo build --release

Requires a Rust toolchain and Windows — the shortcut and icon handling goes
through the Windows Shell COM APIs and has no cross-platform path.

## License

MIT
