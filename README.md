# WinShortcutsOrganizer


<video src="https://github.com/user-attachments/assets/2259a83e-4e2f-4e72-a8e4-99db722ab81f" width="100%" controls></video>


Creates real Windows shortcuts, with correct icons, for the games you already
have installed through Steam and the Epic Games Launcher.

> **No administrator rights are ever required.** `wso` only ever writes to your
> own Desktop and to the per-user Start Menu Programs folder — locations a
> normal user account already owns. If it ever asks you to run it elevated,
> that is a bug, not a requirement.

## Install

Download `wso.exe` from the [releases page](../../releases) and run it. No
installer, no administrator rights, no runtime to install separately.

## Use

Run `wso.exe` with no arguments for an interactive setup wizard that asks
where to place shortcuts, how to organize them, and which platforms to
include.

| Command       | What it does                                                      |
| ------------- | ----------------------------------------------------------------- |
| `wso sync`    | Create and update shortcuts for installed games. Safe to re-run.  |
| `wso migrate` | Convert Steam's broken `.url` shortcuts to real `.lnk` files.     |
| `wso clean`   | Remove every shortcut `wso` created — nothing else.               |
| `wso status`  | Read your library and report what exists and what is out of sync. |

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

| Code | Meaning                                                                                                                                                                                 |
| ---- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `0`  | Every item succeeded.                                                                                                                                                                   |
| `1`  | At least one item failed (an unreadable platform, a target folder that could not be resolved, a shortcut that could not be written) — the run still processed everything else it could. |
| `2`  | The tool could not start at all, before any command logic ran (Windows COM failed to initialize).                                                                                       |

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

`wso --version` reports the version stamped into the binary when it was built.
A build made from a commit tagged `v<version>` reports that version bare
(`wso 0.2.0`); a build from any other commit reports the same version with the
short commit hash it was built from appended (`wso 0.2.0-dev.a1b2c3d`), so a
binary taken from a development branch can never be mistaken for the release it
precedes. Built outside a git checkout — from an unpacked source archive — the
bare version is reported.

## License

MIT
