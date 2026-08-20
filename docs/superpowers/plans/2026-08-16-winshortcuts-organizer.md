# WinShortcutsOrganizer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Windows CLI that generates real `.lnk` shortcuts with correct icons for installed Steam and Epic games, on the Desktop and per-user Start Menu, safely re-runnable and safely reversible.

**Architecture:** Decision logic is pure and side-effect free; COM, registry and filesystem access sit at the edges behind narrow interfaces. A single pure `plan()` function reconciles desired shortcuts against owned ones, and every command is that function with different inputs. Game platforms sit behind a `GameProvider` trait so adding a platform is a new file, not a refactor.

**Tech Stack:** Rust 2021, `clap`, `inquire`, `serde`/`serde_json`, `toml`, `windows`, `image`, `ico`, `thiserror`.

**Spec:** `docs/superpowers/specs/2026-08-16-winshortcutsorganizer-design.md`

## Global Constraints

- Target OS is Windows only. Tests that call Windows APIs are gated with `#[cfg(windows)]`.
- Never require administrator rights. Only `FOLDERID_Desktop` and `FOLDERID_Programs` are written. `FOLDERID_CommonPrograms` is never read or written.
- Never delete or modify a `.lnk` that does not carry our signature.
- All program output, `--help` text, code comments, commit messages and the README are in **English**.
- `appcache/appinfo.vdf` is never parsed. No network access anywhere in the tool.
- Emit `.lnk` files only. Never create a `.url`.
- Binary name is `wso`; crate name is `winshortcutsorganizer`.
- Every destructive operation prints its plan and requires confirmation unless `--yes` is passed.
- Exit codes: `0` all items succeeded, `1` some item failed, `2` global failure.

## Prerequisites

Rust is **not currently installed** on the development machine (verified: `cargo` is absent from `PATH` and `~/.cargo` does not exist). Install it before Task 1:

```bash
winget install --id Rustlang.Rustup -e
```

Then open a new shell and verify:

```bash
cargo --version
```

## File Structure

| File | Responsibility |
|---|---|
| `src/main.rs` | Entry point, COM init, dispatch, exit codes |
| `src/cli.rs` | `clap` command and flag definitions |
| `src/wizard.rs` | Interactive prompts |
| `src/config.rs` | `config.toml`: preferences, exclusions, icon overrides |
| `src/platform/mod.rs` | `Platform`, `Game`, `Launch`, `GameProvider` trait |
| `src/platform/vdf.rs` | Text VDF tokenizer and parser |
| `src/platform/steam.rs` | Steam root discovery, libraries, `.acf` parsing |
| `src/platform/epic.rs` | Epic `.item` manifest parsing |
| `src/shortcut/mod.rs` | `LnkData` model |
| `src/shortcut/writer.rs` | `IShellLinkW` create/read |
| `src/shortcut/signature.rs` | Ownership marker encode/decode |
| `src/shortcut/url_legacy.rs` | Legacy `.url` parsing for `migrate` |
| `src/icon/mod.rs` | `IconCandidate` chain resolution |
| `src/icon/exe_finder.rs` | Executable scoring heuristic (pure) |
| `src/icon/probe.rs` | Does a file contain an icon resource? |
| `src/icon/convert.rs` | PNG to ICO fallback |
| `src/layout.rs` | Known folders, organization modes, name sanitization, collisions |
| `src/plan.rs` | Pure reconciliation |
| `src/ops.rs` | Apply actions, collect per-item outcomes |

Unit tests live in `#[cfg(test)] mod tests` inside each file. Cross-module integration tests live in `tests/`.

---

### Task 1: Scaffold and name sanitization

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/layout.rs`, `.gitignore`

**Interfaces:**
- Consumes: nothing
- Produces: `layout::sanitize_filename(name: &str) -> String`

- [ ] **Step 1: Create the crate**

```bash
cd winshortcutsorganizer
cargo init --name winshortcutsorganizer --bin
```

Then edit `Cargo.toml` so the binary is named `wso`:

```toml
[[bin]]
name = "wso"
path = "src/main.rs"
```

Add to `.gitignore`:

```
/target
```

- [ ] **Step 2: Write the failing test**

Create `src/layout.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_illegal_characters() {
        assert_eq!(sanitize_filename("X4: Foundations"), "X4 Foundations");
        assert_eq!(sanitize_filename("A/B\\C*D?E\"F<G>H|I"), "A B C D E F G H I");
    }

    #[test]
    fn preserves_legal_symbols() {
        assert_eq!(sanitize_filename("Overwatch®"), "Overwatch®");
        assert_eq!(sanitize_filename("HELLDIVERS™ 2"), "HELLDIVERS™ 2");
        assert_eq!(sanitize_filename("Garry's Mod"), "Garry's Mod");
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("Trailmakers."), "Trailmakers");
        assert_eq!(sanitize_filename("Game   "), "Game");
    }

    #[test]
    fn suffixes_reserved_device_names() {
        assert_eq!(sanitize_filename("CON"), "CON_");
        assert_eq!(sanitize_filename("com1"), "com1_");
        assert_eq!(sanitize_filename("CONQUEST"), "CONQUEST");
    }

    #[test]
    fn falls_back_when_nothing_survives() {
        assert_eq!(sanitize_filename("..."), "_");
        assert_eq!(sanitize_filename(""), "_");
    }
}
```

Register the module in `src/main.rs`:

```rust
mod layout;

fn main() {
    println!("wso");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test`
Expected: FAIL — `cannot find function 'sanitize_filename' in this scope`

- [ ] **Step 4: Write minimal implementation**

Prepend to `src/layout.rs`:

```rust
const ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6",
    "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6",
    "lpt7", "lpt8", "lpt9",
];

/// Turns a game title into a filename that NTFS accepts, preserving as much
/// of the original as possible. Trademark symbols are legal and kept.
pub fn sanitize_filename(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if ILLEGAL.contains(&c) || (c as u32) < 0x20 { ' ' } else { c })
        .collect();

    // Collapse runs of whitespace introduced by the replacement above.
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");

    // Windows silently strips trailing dots and spaces; do it explicitly so
    // the name we write matches the name we later read back.
    let trimmed = collapsed.trim_end_matches(['.', ' ']).trim();

    if trimmed.is_empty() {
        return "_".to_string();
    }

    if RESERVED.contains(&trimmed.to_ascii_lowercase().as_str()) {
        return format!("{trimmed}_");
    }

    trimmed.to_string()
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 5 tests

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore src/
git commit -m "feat: scaffold crate and add filename sanitization"
```

---

### Task 2: Ownership signature

**Files:**
- Create: `src/shortcut/mod.rs`, `src/shortcut/signature.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `platform::Platform` enum with variants `Steam`, `Epic`, and `Platform::as_str(&self) -> &'static str`, `Platform::from_str_tag(&str) -> Option<Platform>`
  - `shortcut::signature::Signature { platform: Platform, id: String }`
  - `signature::encode(display_name: &str, sig: &Signature) -> String`
  - `signature::decode(description: &str) -> Option<Signature>`

- [ ] **Step 1: Write the failing test**

Create `src/platform/mod.rs`:

```rust
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
```

Create `src/shortcut/mod.rs`:

```rust
pub mod signature;
```

Create `src/shortcut/signature.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    #[test]
    fn round_trips() {
        let sig = Signature { platform: Platform::Steam, id: "105600".into() };
        let desc = encode("Terraria", &sig);
        assert_eq!(
            desc,
            "Terraria — managed by WinShortcutsOrganizer [wso:steam:105600]"
        );
        assert_eq!(decode(&desc), Some(sig));
    }

    #[test]
    fn decodes_epic_alphanumeric_ids() {
        let sig = Signature { platform: Platform::Epic, id: "Fortnite".into() };
        assert_eq!(decode(&encode("Fortnite", &sig)), Some(sig));
    }

    #[test]
    fn rejects_foreign_descriptions() {
        assert_eq!(decode(""), None);
        assert_eq!(decode("My own shortcut"), None);
        assert_eq!(decode("Some game [wso:gog:123]"), None);
        assert_eq!(decode("Some game [wso:steam]"), None);
        assert_eq!(decode("Some game [wso:steam:]"), None);
        assert_eq!(decode("[wso:steam:440] leading marker"), None);
    }

    #[test]
    fn ignores_brackets_earlier_in_the_name() {
        let sig = Signature { platform: Platform::Steam, id: "440".into() };
        let desc = encode("Team Fortress 2 [Beta]", &sig);
        assert_eq!(decode(&desc), Some(sig));
    }
}
```

Register modules in `src/main.rs`:

```rust
mod layout;
mod platform;
mod shortcut;

fn main() {
    println!("wso");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test signature`
Expected: FAIL — `cannot find type 'Signature' in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/shortcut/signature.rs`:

```rust
use crate::platform::Platform;

const PREFIX: &str = "[wso:";
const SUFFIX: char = ']';

/// Identifies a shortcut this tool owns. Stored in the `.lnk` Description
/// field, which Windows shows as the hover tooltip, so it is written to be
/// readable by humans as well as by us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub platform: Platform,
    pub id: String,
}

pub fn encode(display_name: &str, sig: &Signature) -> String {
    format!(
        "{display_name} — managed by WinShortcutsOrganizer [wso:{}:{}]",
        sig.platform.as_str(),
        sig.id
    )
}

pub fn decode(description: &str) -> Option<Signature> {
    let trimmed = description.trim_end();
    if !trimmed.ends_with(SUFFIX) {
        return None;
    }
    // Search from the right so brackets earlier in the title cannot confuse us.
    let open = trimmed.rfind(PREFIX)?;
    let body = &trimmed[open + PREFIX.len()..trimmed.len() - 1];

    let (tag, id) = body.split_once(':')?;
    if id.is_empty() || id.contains(':') {
        return None;
    }

    Some(Signature {
        platform: Platform::from_str_tag(tag)?,
        id: id.to_string(),
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 9 tests

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "feat: add ownership signature encoding for managed shortcuts"
```

---

### Task 3: Text VDF parser

**Files:**
- Create: `src/platform/vdf.rs`
- Modify: `src/platform/mod.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `platform::vdf::Value` enum with variants `Str(String)` and `Obj(Vec<(String, Value)>)`
  - `platform::vdf::parse(input: &str) -> Result<Value, VdfError>`
  - `Value::get(&self, key: &str) -> Option<&Value>` — case-insensitive key lookup
  - `Value::as_str(&self) -> Option<&str>`
  - `Value::entries(&self) -> &[(String, Value)]`

- [ ] **Step 1: Write the failing test**

Create `src/platform/vdf.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARYFOLDERS: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps"
		{
			"228980"		"262322053"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"apps"
		{
			"4000"		"7258400897"
			"105600"		"801432759"
		}
	}
}
"#;

    const APPMANIFEST: &str = r#"
"AppState"
{
	"appid"		"105600"
	"name"		"Terraria"
	"installdir"		"Terraria"
	"StateFlags"		"4"
}
"#;

    #[test]
    fn parses_nested_library_folders() {
        let v = parse(LIBRARYFOLDERS).unwrap();
        let root = v.get("libraryfolders").unwrap();
        assert_eq!(root.entries().len(), 2);
        assert_eq!(
            root.get("1").unwrap().get("path").unwrap().as_str(),
            Some(r"D:\SteamLibrary")
        );
        assert_eq!(root.get("1").unwrap().get("apps").unwrap().entries().len(), 2);
    }

    #[test]
    fn parses_app_manifest_fields() {
        let v = parse(APPMANIFEST).unwrap();
        let state = v.get("AppState").unwrap();
        assert_eq!(state.get("appid").unwrap().as_str(), Some("105600"));
        assert_eq!(state.get("name").unwrap().as_str(), Some("Terraria"));
        assert_eq!(state.get("installdir").unwrap().as_str(), Some("Terraria"));
    }

    #[test]
    fn key_lookup_is_case_insensitive() {
        let v = parse(APPMANIFEST).unwrap();
        assert_eq!(
            v.get("appstate").unwrap().get("APPID").unwrap().as_str(),
            Some("105600")
        );
    }

    #[test]
    fn skips_line_comments() {
        let v = parse("\"root\"\n{\n// a comment\n\"k\" \"v\"\n}\n").unwrap();
        assert_eq!(v.get("root").unwrap().get("k").unwrap().as_str(), Some("v"));
    }

    #[test]
    fn reports_unterminated_object() {
        assert!(matches!(parse("\"root\"\n{\n"), Err(VdfError::UnexpectedEnd)));
    }

    #[test]
    fn reports_unterminated_string() {
        assert!(matches!(parse("\"root"), Err(VdfError::UnexpectedEnd)));
    }
}
```

Add to `src/platform/mod.rs`:

```rust
pub mod vdf;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test vdf`
Expected: FAIL — `cannot find function 'parse' in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/platform/vdf.rs`:

```rust
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum VdfError {
    UnexpectedEnd,
    UnexpectedToken(String),
}

impl fmt::Display for VdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VdfError::UnexpectedEnd => write!(f, "unexpected end of VDF input"),
            VdfError::UnexpectedToken(t) => write!(f, "unexpected token in VDF: {t}"),
        }
    }
}

impl std::error::Error for VdfError {}

#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(entries) => entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Value::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Obj(entries) => entries,
            Value::Str(_) => &[],
        }
    }
}

/// Parses Steam's text keyvalues format, as used by `libraryfolders.vdf` and
/// `appmanifest_*.acf`. The binary VDF variant is deliberately not supported.
pub fn parse(input: &str) -> Result<Value, VdfError> {
    let mut p = Parser { bytes: input.as_bytes(), pos: 0 };
    let entries = p.parse_entries(true)?;
    Ok(Value::Obj(entries))
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn parse_entries(&mut self, top_level: bool) -> Result<Vec<(String, Value)>, VdfError> {
        let mut entries = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => {
                    if top_level {
                        return Ok(entries);
                    }
                    return Err(VdfError::UnexpectedEnd);
                }
                Some(b'}') => {
                    if top_level {
                        return Err(VdfError::UnexpectedToken("}".into()));
                    }
                    self.pos += 1;
                    return Ok(entries);
                }
                Some(b'"') => {
                    let key = self.parse_string()?;
                    self.skip_trivia();
                    match self.peek() {
                        Some(b'"') => entries.push((key, Value::Str(self.parse_string()?))),
                        Some(b'{') => {
                            self.pos += 1;
                            entries.push((key, Value::Obj(self.parse_entries(false)?)));
                        }
                        None => return Err(VdfError::UnexpectedEnd),
                        Some(c) => {
                            return Err(VdfError::UnexpectedToken((c as char).to_string()))
                        }
                    }
                }
                Some(c) => return Err(VdfError::UnexpectedToken((c as char).to_string())),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, VdfError> {
        // Caller guarantees the current byte is an opening quote.
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(VdfError::UnexpectedEnd),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        None => return Err(VdfError::UnexpectedEnd),
                        Some(b'n') => out.push('\n'),
                        Some(b't') => out.push('\t'),
                        Some(c) => out.push(c as char),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    // Advance one full UTF-8 character so non-ASCII titles survive.
                    let rest = &self.bytes[self.pos..];
                    let s = std::str::from_utf8(rest).map_err(|_| VdfError::UnexpectedEnd)?;
                    let ch = s.chars().next().ok_or(VdfError::UnexpectedEnd)?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.pos += 1;
            }
            if self.bytes[self.pos..].starts_with(b"//") {
                while !matches!(self.peek(), None | Some(b'\n')) {
                    self.pos += 1;
                }
                continue;
            }
            return;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 15 tests

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "feat: add text VDF parser for Steam manifests"
```

---

### Task 4: Executable finder heuristic

This is the load-bearing algorithm. Steam's manifests say which folder a game
lives in but not which `.exe` is the game. The rule — shallowest depth first,
blocklist, then name match, then size — was derived from a real nine-game
library, and the fixtures below are that library. Two of them break the naive
"largest executable wins" rule and must keep passing.

**Files:**
- Create: `src/icon/mod.rs`, `src/icon/exe_finder.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `icon::exe_finder::ExeCandidate { rel_path: PathBuf, size: u64 }`
  - `ExeCandidate::depth(&self) -> usize` — path components including the filename
  - `icon::exe_finder::pick_executable(game_name: &str, install_dir_name: &str, candidates: &[ExeCandidate]) -> Option<&ExeCandidate>`
  - `icon::exe_finder::collect_candidates(install_dir: &Path, max_depth: usize) -> Vec<ExeCandidate>`

- [ ] **Step 1: Write the failing test**

Create `src/icon/exe_finder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn c(path: &str, size: u64) -> ExeCandidate {
        ExeCandidate { rel_path: PathBuf::from(path), size }
    }

    fn pick<'a>(name: &str, dir: &str, cands: &'a [ExeCandidate]) -> &'a str {
        pick_executable(name, dir, cands)
            .expect("expected a candidate")
            .rel_path
            .to_str()
            .unwrap()
    }

    #[test]
    fn terraria_prefers_the_game_over_the_server() {
        let cands = [
            c("Terraria.exe", 25_990_656),
            c("TerrariaServer.exe", 25_420_288),
            c("dotNetFx40_Full_x86_x64.exe", 1_600_220),
            c("dotNetFx40_Full_setup.exe", 889_416),
        ];
        assert_eq!(pick("Terraria", "Terraria", &cands), "Terraria.exe");
    }

    #[test]
    fn persona_3_reload_descends_into_unreal_layout() {
        let cands = [
            c(r"P3R\Binaries\Win64\P3R.exe", 366_251_968),
            c(r"Engine\Binaries\Win64\CrashReportClient.exe", 19_487_232),
        ];
        assert_eq!(
            pick("Persona 3 Reload", "P3R", &cands),
            r"P3R\Binaries\Win64\P3R.exe"
        );
    }

    #[test]
    fn monster_hunter_wilds_ignores_a_huge_crash_reporter() {
        let cands = [
            c("MonsterHunterWilds.exe", 539_469_728),
            c("CrashReport.exe", 155_125_104),
            c("InstallerMessage.exe", 1_623_120),
        ];
        assert_eq!(
            pick("Monster Hunter Wilds", "MonsterHunterWilds", &cands),
            "MonsterHunterWilds.exe"
        );
    }

    #[test]
    fn overwatch_ignores_nested_helpers() {
        let cands = [
            c("Overwatch.exe", 65_078_992),
            c(r"BlizzardBrowser\BlizzardBrowser.exe", 2_267_856),
            c(r"ErrorReporting\x86\BlizzardError.exe", 908_416),
            c(r"ErrorReporting\x64\CrashMailer_64.exe", 450_768),
        ];
        assert_eq!(pick("Overwatch®", "Overwatch", &cands), "Overwatch.exe");
    }

    #[test]
    fn x4_matches_on_a_partial_name() {
        let cands = [
            c("X4.exe", 55_412_768),
            c(r"support\drdump\sendrpt.exe", 911_872),
        ];
        assert_eq!(pick("X4: Foundations", "X4 Foundations", &cands), "X4.exe");
    }

    /// Regression: the largest executable is a Source modelling tool three
    /// levels deep. Depth must outrank size.
    #[test]
    fn garrys_mod_prefers_the_shallow_launcher_over_a_bigger_tool() {
        let cands = [
            c("gmod.exe", 430_928),
            c(r"bin\win64\studiomdl.exe", 2_818_384),
            c(r"bin\studiomdl.exe", 2_256_208),
            c(r"bin\hlfaceposer.exe", 2_121_552),
            c(r"bin\win64\vbsp.exe", 1_437_520),
            c(r"bin\win64\gmod.exe", 1_042_256),
        ];
        assert_eq!(pick("Garry's Mod", "GarrysMod", &cands), "gmod.exe");
    }

    #[test]
    fn helldivers_2_beats_a_similarly_sized_installer() {
        let cands = [
            c(r"bin\helldivers2.exe", 14_892_136),
            c(r"tools\GGSetup.exe", 13_163_048),
            c(r"bin\crs-handler.exe", 1_188_520),
            c(r"tools\gguninst.exe", 79_928),
        ];
        assert_eq!(
            pick("HELLDIVERS™ 2", "Helldivers 2", &cands),
            r"bin\helldivers2.exe"
        );
    }

    /// Regression: Unity's crash handler is physically larger than the game.
    #[test]
    fn supermarket_together_ignores_the_unity_crash_handler() {
        let cands = [
            c("UnityCrashHandler64.exe", 991_992),
            c("Supermarket Together.exe", 667_648),
        ];
        assert_eq!(
            pick("Supermarket Together", "Supermarket Together", &cands),
            "Supermarket Together.exe"
        );
    }

    #[test]
    fn returns_none_when_everything_is_blocklisted() {
        let cands = [c("vcredist_x64.exe", 100), c("UnityCrashHandler64.exe", 200)];
        assert!(pick_executable("Some Game", "SomeGame", &cands).is_none());
    }

    #[test]
    fn returns_none_for_an_empty_directory() {
        assert!(pick_executable("Some Game", "SomeGame", &[]).is_none());
    }

    #[test]
    fn depth_counts_the_filename() {
        assert_eq!(c("a.exe", 1).depth(), 1);
        assert_eq!(c(r"bin\a.exe", 1).depth(), 2);
        assert_eq!(c(r"a\b\c\d.exe", 1).depth(), 4);
    }
}
```

Create `src/icon/mod.rs`:

```rust
pub mod exe_finder;
```

Add `mod icon;` to `src/main.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test exe_finder`
Expected: FAIL — `cannot find struct 'ExeCandidate' in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/icon/exe_finder.rs`:

```rust
use std::path::{Path, PathBuf};

/// Filename fragments that never identify a game. Matched case-insensitively
/// against the file stem.
const NAME_BLOCKLIST: [&str; 10] = [
    "crashhandler",
    "crashreport",
    "crashmailer",
    "setup",
    "installer",
    "uninst",
    "dotnetfx",
    "vcredist",
    "dxsetup",
    "sendrpt",
];

/// Directory names that only ever hold redistributables.
const DIR_BLOCKLIST: [&str; 5] = [
    "_commonredist",
    "redist",
    "directx",
    "dotnet",
    "vcredist",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeCandidate {
    pub rel_path: PathBuf,
    pub size: u64,
}

impl ExeCandidate {
    /// Number of path components, counting the filename. A file directly in
    /// the install directory has depth 1.
    pub fn depth(&self) -> usize {
        self.rel_path.components().count()
    }

    fn stem_lower(&self) -> String {
        self.rel_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    }

    fn is_blocklisted(&self) -> bool {
        let stem = self.stem_lower();
        if NAME_BLOCKLIST.iter().any(|b| stem.contains(b)) {
            return true;
        }
        self.rel_path
            .parent()
            .map(|p| {
                p.components().any(|c| {
                    let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
                    DIR_BLOCKLIST.contains(&s.as_str())
                })
            })
            .unwrap_or(false)
    }
}

/// Reduces a title to comparable letters and digits, so "Garry's Mod",
/// "GarrysMod" and "garrys mod" all normalize to the same thing.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Higher is better. 0 means no relationship at all.
fn name_score(candidate_stem: &str, game_name: &str, install_dir_name: &str) -> u32 {
    let stem = normalize(candidate_stem);
    if stem.is_empty() {
        return 0;
    }
    let mut best = 0;
    for target in [normalize(game_name), normalize(install_dir_name)] {
        if target.is_empty() {
            continue;
        }
        let score = if stem == target {
            100
        } else if stem.starts_with(&target) || target.starts_with(&stem) {
            60
        } else if stem.contains(&target) || target.contains(&stem) {
            40
        } else {
            0
        };
        best = best.max(score);
    }
    best
}

/// Picks the executable whose icon best represents the game.
///
/// Depth dominates: a small launcher in the install root beats a large tool
/// buried in `bin\win64\`. Within the shallowest populated depth, name
/// similarity decides, then file size.
pub fn pick_executable<'a>(
    game_name: &str,
    install_dir_name: &str,
    candidates: &'a [ExeCandidate],
) -> Option<&'a ExeCandidate> {
    let viable: Vec<&ExeCandidate> =
        candidates.iter().filter(|c| !c.is_blocklisted()).collect();

    let min_depth = viable.iter().map(|c| c.depth()).min()?;

    viable
        .into_iter()
        .filter(|c| c.depth() == min_depth)
        .max_by_key(|c| (name_score(&c.stem_lower(), game_name, install_dir_name), c.size))
}

/// Walks an install directory collecting `.exe` files, bounded by depth so a
/// game with a deep asset tree does not cost a full recursive scan.
pub fn collect_candidates(install_dir: &Path, max_depth: usize) -> Vec<ExeCandidate> {
    let mut out = Vec::new();
    walk(install_dir, install_dir, max_depth, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, remaining: usize, out: &mut Vec<ExeCandidate>) {
    if remaining == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk(root, &path, remaining - 1, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(ExeCandidate { rel_path: rel.to_path_buf(), size: meta.len() });
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 26 tests. All eight game fixtures must pass; if Garry's Mod or
Supermarket Together fails, the depth or blocklist rule has been weakened.

- [ ] **Step 5: Add an integration test for the directory walk**

Create `tests/exe_walk.rs`:

```rust
use std::fs;
use winshortcutsorganizer::icon::exe_finder::{collect_candidates, pick_executable};

#[test]
fn walks_a_real_directory_and_picks_the_game() {
    let tmp = std::env::temp_dir().join("wso_exe_walk_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("bin").join("win64")).unwrap();
    fs::write(tmp.join("gmod.exe"), vec![0u8; 400]).unwrap();
    fs::write(tmp.join("bin").join("win64").join("studiomdl.exe"), vec![0u8; 4000]).unwrap();
    fs::write(tmp.join("readme.txt"), b"not an exe").unwrap();

    let cands = collect_candidates(&tmp, 4);
    assert_eq!(cands.len(), 2, "only .exe files are collected");

    let picked = pick_executable("Garry's Mod", "GarrysMod", &cands).unwrap();
    assert_eq!(picked.rel_path.file_name().unwrap(), "gmod.exe");

    fs::remove_dir_all(&tmp).unwrap();
}
```

This requires a library target. Create `src/lib.rs`:

```rust
pub mod icon;
pub mod layout;
pub mod platform;
pub mod plan;
pub mod shortcut;
```

and change `src/main.rs` to use it:

```rust
use winshortcutsorganizer as wso;

fn main() {
    let _ = wso::layout::sanitize_filename("wso");
    println!("wso");
}
```

Note: `src/plan.rs` does not exist yet. Create it as an empty file with a
placeholder module doc so `lib.rs` compiles:

```rust
//! Pure reconciliation between desired and owned shortcuts. Implemented in
//! Task 9.
```

- [ ] **Step 6: Run the integration test**

Run: `cargo test --test exe_walk`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/ tests/
git commit -m "feat: add executable finder heuristic with real-library fixtures"
```

---

### Task 5: Game model and Steam provider

**Files:**
- Modify: `src/platform/mod.rs`
- Create: `src/platform/steam.rs`, `tests/steam_provider.rs`

**Interfaces:**
- Consumes: `platform::vdf`, `platform::Platform`
- Produces:
  - `platform::Launch { program: PathBuf, args: String, working_dir: Option<PathBuf> }`
  - `platform::IconCandidate` enum: `Executable(PathBuf)`, `ImageFile(PathBuf)`, `Fallback(PathBuf)`
  - `platform::Game { platform: Platform, id: String, name: String, install_dir: PathBuf, launch: Launch, icon_candidates: Vec<IconCandidate> }`
  - `platform::GameProvider` trait with `platform()`, `games()`
  - `platform::steam::SteamProvider::detect() -> Option<SteamProvider>`
  - `platform::steam::SteamProvider::from_root(root: PathBuf) -> SteamProvider`
  - `platform::steam::SteamProvider::libraries(&self) -> Result<Vec<PathBuf>, ProviderError>`

- [ ] **Step 1: Extend the shared model**

Append to `src/platform/mod.rs`:

```rust
pub mod steam;

use std::path::PathBuf;

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
```

- [ ] **Step 2: Write the failing test**

Create `src/platform/steam.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_paths() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}
"#;
        let libs = parse_library_paths(vdf).unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[1], PathBuf::from(r"D:\SteamLibrary"));
    }

    #[test]
    fn parses_an_app_manifest() {
        let acf = r#"
"AppState"
{
	"appid"		"105600"
	"name"		"Terraria"
	"installdir"		"Terraria"
}
"#;
        let app = parse_app_manifest(acf).unwrap();
        assert_eq!(app.appid, "105600");
        assert_eq!(app.name, "Terraria");
        assert_eq!(app.installdir, "Terraria");
    }

    #[test]
    fn rejects_a_manifest_missing_fields() {
        assert!(parse_app_manifest("\"AppState\"\n{\n\"appid\" \"1\"\n}\n").is_none());
    }

    #[test]
    fn filters_infrastructure_by_appid() {
        assert!(is_infrastructure("228980", "Steamworks Common Redistributables"));
        assert!(is_infrastructure("1070560", "Steam Linux Runtime"));
        assert!(!is_infrastructure("105600", "Terraria"));
    }

    #[test]
    fn filters_infrastructure_by_name() {
        assert!(is_infrastructure("999999", "Proton 9.0"));
        assert!(is_infrastructure("999998", "Steam Linux Runtime 3.0 (sniper)"));
        assert!(is_infrastructure("999997", "Steamworks Shared"));
        assert!(!is_infrastructure("999996", "Protonwall"));
    }

    #[test]
    fn builds_a_launch_command() {
        let p = SteamProvider::from_root(PathBuf::from(r"C:\Steam"));
        let launch = p.launch_for("440");
        assert_eq!(launch.program, PathBuf::from(r"C:\Steam\steam.exe"));
        assert_eq!(launch.args, "-applaunch 440");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test steam`
Expected: FAIL — `cannot find function 'parse_library_paths' in this scope`

- [ ] **Step 4: Write minimal implementation**

Prepend to `src/platform/steam.rs`:

```rust
use super::vdf;
use super::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError};
use crate::icon::exe_finder;
use std::path::{Path, PathBuf};

/// Appids that ship with Steam but are not games.
const INFRA_APPIDS: [&str; 6] =
    ["228980", "1070560", "1391110", "1493710", "1580130", "1826330"];

/// Name prefixes that identify runtimes and compatibility tools.
const INFRA_NAME_PREFIXES: [&str; 3] =
    ["proton ", "steam linux runtime", "steamworks "];

pub struct SteamProvider {
    root: PathBuf,
}

pub(crate) struct AppManifest {
    pub appid: String,
    pub name: String,
    pub installdir: String,
}

impl SteamProvider {
    /// Resolves Steam's install directory. The registry is tried first, but
    /// the conventional paths are a required fallback, not a nicety: on the
    /// machine this was validated against, `HKCU\Software\Valve\Steam` did not
    /// answer while Steam was correctly installed.
    pub fn detect() -> Option<SteamProvider> {
        for candidate in registry_roots().into_iter().chain(conventional_roots()) {
            if candidate.join("steam.exe").is_file() {
                return Some(SteamProvider { root: candidate });
            }
        }
        None
    }

    pub fn from_root(root: PathBuf) -> SteamProvider {
        SteamProvider { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn launch_for(&self, appid: &str) -> Launch {
        Launch {
            program: self.root.join("steam.exe"),
            args: format!("-applaunch {appid}"),
            working_dir: Some(self.root.clone()),
        }
    }

    pub fn libraries(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let path = self.root.join("steamapps").join("libraryfolders.vdf");
        let text = std::fs::read_to_string(&path).map_err(|e| ProviderError::Unreadable {
            path: path.clone(),
            detail: e.to_string(),
        })?;
        parse_library_paths(&text).ok_or(ProviderError::Unreadable {
            path,
            detail: "no library paths found".into(),
        })
    }
}

pub(crate) fn parse_library_paths(text: &str) -> Option<Vec<PathBuf>> {
    let root = vdf::parse(text).ok()?;
    let folders = root.get("libraryfolders")?;
    let paths: Vec<PathBuf> = folders
        .entries()
        .iter()
        .filter_map(|(_, v)| v.get("path")?.as_str().map(PathBuf::from))
        .collect();
    if paths.is_empty() {
        None
    } else {
        Some(paths)
    }
}

pub(crate) fn parse_app_manifest(text: &str) -> Option<AppManifest> {
    let root = vdf::parse(text).ok()?;
    let state = root.get("AppState")?;
    Some(AppManifest {
        appid: state.get("appid")?.as_str()?.to_string(),
        name: state.get("name")?.as_str()?.to_string(),
        installdir: state.get("installdir")?.as_str()?.to_string(),
    })
}

pub(crate) fn is_infrastructure(appid: &str, name: &str) -> bool {
    if INFRA_APPIDS.contains(&appid) {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    INFRA_NAME_PREFIXES.iter().any(|p| lower.starts_with(p))
}

fn conventional_roots() -> Vec<PathBuf> {
    let mut out = vec![
        PathBuf::from(r"C:\Program Files (x86)\Steam"),
        PathBuf::from(r"C:\Program Files\Steam"),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        out.push(PathBuf::from(local).join("Steam"));
    }
    out
}

impl GameProvider for SteamProvider {
    fn platform(&self) -> Platform {
        Platform::Steam
    }

    fn games(&self) -> Result<Vec<Game>, ProviderError> {
        let mut games = Vec::new();
        for library in self.libraries()? {
            let steamapps = library.join("steamapps");
            let Ok(entries) = std::fs::read_dir(&steamapps) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_manifest = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf"));
                if !is_manifest {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Some(app) = parse_app_manifest(&text) else {
                    continue;
                };
                if is_infrastructure(&app.appid, &app.name) {
                    continue;
                }
                let install_dir = steamapps.join("common").join(&app.installdir);
                if !install_dir.is_dir() {
                    continue;
                }
                games.push(self.build_game(app, install_dir));
            }
        }
        games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(games)
    }
}

impl SteamProvider {
    fn build_game(&self, app: AppManifest, install_dir: PathBuf) -> Game {
        let candidates = exe_finder::collect_candidates(&install_dir, 4);
        let mut icons = Vec::new();
        if let Some(exe) = exe_finder::pick_executable(&app.name, &app.installdir, &candidates) {
            icons.push(IconCandidate::Executable(install_dir.join(&exe.rel_path)));
        }
        icons.push(IconCandidate::ImageFile(
            self.root
                .join("appcache")
                .join("librarycache")
                .join(&app.appid)
                .join("logo.png"),
        ));
        icons.push(IconCandidate::Fallback(self.root.join("steam.exe")));

        Game {
            platform: Platform::Steam,
            launch: self.launch_for(&app.appid),
            id: app.appid,
            name: app.name,
            install_dir,
            icon_candidates: icons,
        }
    }
}
```

Add a Windows-only registry lookup at the end of the file:

```rust
#[cfg(windows)]
fn registry_roots() -> Vec<PathBuf> {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let mut out = Vec::new();
    let queries = [
        (HKEY_CURRENT_USER, w!("Software\\Valve\\Steam"), w!("SteamPath")),
        (
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\WOW6432Node\\Valve\\Steam"),
            w!("InstallPath"),
        ),
    ];

    for (hkey, subkey, value) in queries {
        let mut buf = [0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                hkey,
                subkey,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut size),
            )
        };
        if status.is_ok() {
            let len = (size as usize / 2).saturating_sub(1);
            let s = String::from_utf16_lossy(&buf[..len]);
            if !s.is_empty() {
                out.push(PathBuf::from(s));
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_roots() -> Vec<PathBuf> {
    Vec::new()
}
```

Add the dependency:

```bash
cargo add windows --features Win32_System_Registry,Win32_Foundation
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 32 tests

- [ ] **Step 6: Write an integration test over a synthetic Steam tree**

Create `tests/steam_provider.rs`:

```rust
use std::fs;
use std::path::PathBuf;
use winshortcutsorganizer::platform::{steam::SteamProvider, GameProvider};

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn discovers_games_and_filters_infrastructure() {
    let tmp = std::env::temp_dir().join("wso_steam_provider_test");
    let _ = fs::remove_dir_all(&tmp);
    let root = tmp.join("Steam");

    write(
        root.join("steamapps").join("libraryfolders.vdf"),
        &format!(
            "\"libraryfolders\"\n{{\n\"0\"\n{{\n\"path\"\t\t\"{}\"\n}}\n}}\n",
            root.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    write(
        root.join("steamapps").join("appmanifest_105600.acf"),
        "\"AppState\"\n{\n\"appid\"\t\"105600\"\n\"name\"\t\"Terraria\"\n\"installdir\"\t\"Terraria\"\n}\n",
    );
    write(
        root.join("steamapps").join("appmanifest_228980.acf"),
        "\"AppState\"\n{\n\"appid\"\t\"228980\"\n\"name\"\t\"Steamworks Common Redistributables\"\n\"installdir\"\t\"Steamworks Shared\"\n}\n",
    );
    fs::create_dir_all(root.join("steamapps").join("common").join("Terraria")).unwrap();
    fs::create_dir_all(root.join("steamapps").join("common").join("Steamworks Shared")).unwrap();
    fs::write(
        root.join("steamapps").join("common").join("Terraria").join("Terraria.exe"),
        vec![0u8; 100],
    )
    .unwrap();

    let games = SteamProvider::from_root(root.clone()).games().unwrap();

    assert_eq!(games.len(), 1, "infrastructure appid must be filtered out");
    assert_eq!(games[0].name, "Terraria");
    assert_eq!(games[0].id, "105600");
    assert_eq!(games[0].launch.args, "-applaunch 105600");
    assert_eq!(games[0].launch.program, root.join("steam.exe"));
    assert_eq!(games[0].icon_candidates.len(), 3, "exe, librarycache, fallback");

    fs::remove_dir_all(&tmp).unwrap();
}
```

- [ ] **Step 7: Run the integration test**

Run: `cargo test --test steam_provider`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/ tests/ Cargo.toml Cargo.lock
git commit -m "feat: add Steam provider with library and manifest discovery"
```

---

### Task 6: Epic provider

**Files:**
- Create: `src/platform/epic.rs`, `tests/epic_provider.rs`
- Modify: `src/platform/mod.rs`

**Interfaces:**
- Consumes: `platform::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError}`
- Produces:
  - `platform::epic::EpicProvider::detect() -> Option<EpicProvider>`
  - `platform::epic::EpicProvider::from_dirs(manifests_dir: PathBuf, launcher_exe: PathBuf) -> EpicProvider`
  - `platform::epic::launch_uri(namespace: &str, catalog_item_id: &str, app_name: &str) -> String`

- [ ] **Step 1: Write the failing test**

Create `src/platform/epic.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const GAME_ITEM: &str = r#"{
        "DisplayName": "Fall Guys",
        "InstallLocation": "D:\\Epic\\FallGuys",
        "LaunchExecutable": "FallGuys_client.exe",
        "AppName": "9c88",
        "CatalogNamespace": "0a2d9f",
        "CatalogItemId": "abc123",
        "AppCategories": ["public", "games", "applications"],
        "bIsIncompleteInstall": false
    }"#;

    const DLC_ITEM: &str = r#"{
        "DisplayName": "Some Addon",
        "InstallLocation": "D:\\Epic\\Addon",
        "LaunchExecutable": "",
        "AppName": "addon1",
        "CatalogNamespace": "ns",
        "CatalogItemId": "cid",
        "AppCategories": ["public", "addons"],
        "bIsIncompleteInstall": false
    }"#;

    const INCOMPLETE_ITEM: &str = r#"{
        "DisplayName": "Half Downloaded",
        "InstallLocation": "D:\\Epic\\Half",
        "LaunchExecutable": "half.exe",
        "AppName": "half1",
        "CatalogNamespace": "ns",
        "CatalogItemId": "cid",
        "AppCategories": ["games"],
        "bIsIncompleteInstall": true
    }"#;

    #[test]
    fn accepts_a_game_manifest() {
        let m: Manifest = serde_json::from_str(GAME_ITEM).unwrap();
        assert!(m.is_installed_game());
        assert_eq!(m.display_name, "Fall Guys");
    }

    #[test]
    fn rejects_non_game_categories() {
        let m: Manifest = serde_json::from_str(DLC_ITEM).unwrap();
        assert!(!m.is_installed_game());
    }

    #[test]
    fn rejects_incomplete_installs() {
        let m: Manifest = serde_json::from_str(INCOMPLETE_ITEM).unwrap();
        assert!(!m.is_installed_game());
    }

    #[test]
    fn tolerates_unknown_fields() {
        let json = r#"{
            "DisplayName": "X", "InstallLocation": "C:\\X",
            "LaunchExecutable": "x.exe", "AppName": "x",
            "CatalogNamespace": "n", "CatalogItemId": "c",
            "AppCategories": ["games"], "SomethingEpicAddedLater": 42
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.is_installed_game());
    }

    #[test]
    fn builds_the_launch_uri_with_encoded_separators() {
        assert_eq!(
            launch_uri("0a2d9f", "abc123", "9c88"),
            "com.epicgames.launcher://apps/0a2d9f%3Aabc123%3A9c88?action=launch&silent=true"
        );
    }
}
```

Add `pub mod epic;` to `src/platform/mod.rs`, then:

```bash
cargo add serde --features derive
cargo add serde_json
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test epic`
Expected: FAIL — `cannot find type 'Manifest' in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/platform/epic.rs`:

```rust
use super::{Game, GameProvider, IconCandidate, Launch, Platform, ProviderError};
use serde::Deserialize;
use std::path::PathBuf;

/// One entry from Epic's manifest directory. Unknown fields are ignored so a
/// launcher update that adds keys does not break parsing.
#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    #[serde(rename = "DisplayName")]
    pub display_name: String,
    #[serde(rename = "InstallLocation")]
    pub install_location: String,
    #[serde(rename = "LaunchExecutable")]
    pub launch_executable: String,
    #[serde(rename = "AppName")]
    pub app_name: String,
    #[serde(rename = "CatalogNamespace")]
    pub catalog_namespace: String,
    #[serde(rename = "CatalogItemId")]
    pub catalog_item_id: String,
    #[serde(rename = "AppCategories", default)]
    pub app_categories: Vec<String>,
    #[serde(rename = "bIsIncompleteInstall", default)]
    pub incomplete: bool,
}

impl Manifest {
    /// Keeps only playable, fully installed titles. This is what excludes
    /// DLC, plugins and Unreal Engine installs.
    pub(crate) fn is_installed_game(&self) -> bool {
        !self.incomplete
            && !self.launch_executable.is_empty()
            && self
                .app_categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case("games"))
    }
}

/// A `.lnk` cannot target a protocol URI, so the shortcut runs Explorer and
/// hands it the URI. Launching through the URI rather than the executable
/// preserves Epic's own launch handling and entitlement checks.
pub(crate) fn launch_uri(namespace: &str, catalog_item_id: &str, app_name: &str) -> String {
    format!(
        "com.epicgames.launcher://apps/{namespace}%3A{catalog_item_id}%3A{app_name}?action=launch&silent=true"
    )
}

pub struct EpicProvider {
    manifests_dir: PathBuf,
    launcher_exe: PathBuf,
}

impl EpicProvider {
    pub fn detect() -> Option<EpicProvider> {
        let manifests = PathBuf::from(r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests");
        if !manifests.is_dir() {
            return None;
        }
        Some(EpicProvider {
            manifests_dir: manifests,
            launcher_exe: PathBuf::from(
                r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win32\EpicGamesLauncher.exe",
            ),
        })
    }

    pub fn from_dirs(manifests_dir: PathBuf, launcher_exe: PathBuf) -> EpicProvider {
        EpicProvider { manifests_dir, launcher_exe }
    }

    fn explorer() -> PathBuf {
        std::env::var("WINDIR")
            .map(|w| PathBuf::from(w).join("explorer.exe"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\explorer.exe"))
    }
}

impl GameProvider for EpicProvider {
    fn platform(&self) -> Platform {
        Platform::Epic
    }

    fn games(&self) -> Result<Vec<Game>, ProviderError> {
        let entries =
            std::fs::read_dir(&self.manifests_dir).map_err(|e| ProviderError::Unreadable {
                path: self.manifests_dir.clone(),
                detail: e.to_string(),
            })?;

        let mut games = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("item") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(m) = serde_json::from_str::<Manifest>(&text) else {
                continue;
            };
            if !m.is_installed_game() {
                continue;
            }

            let install_dir = PathBuf::from(&m.install_location);
            let exe = install_dir.join(&m.launch_executable);

            games.push(Game {
                platform: Platform::Epic,
                id: m.app_name.clone(),
                name: m.display_name.clone(),
                launch: Launch {
                    program: Self::explorer(),
                    args: format!(
                        "\"{}\"",
                        launch_uri(&m.catalog_namespace, &m.catalog_item_id, &m.app_name)
                    ),
                    working_dir: None,
                },
                // Epic declares its executable, so no heuristic is needed here.
                icon_candidates: vec![
                    IconCandidate::Executable(exe),
                    IconCandidate::Fallback(self.launcher_exe.clone()),
                ],
                install_dir,
            });
        }
        games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(games)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 37 tests

- [ ] **Step 5: Write an integration test**

Create `tests/epic_provider.rs`:

```rust
use std::fs;
use std::path::PathBuf;
use winshortcutsorganizer::platform::{epic::EpicProvider, GameProvider};

#[test]
fn reads_manifests_and_filters_non_games() {
    let tmp = std::env::temp_dir().join("wso_epic_provider_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    fs::write(
        tmp.join("game.item"),
        r#"{"DisplayName":"Fall Guys","InstallLocation":"D:\\Epic\\FallGuys",
            "LaunchExecutable":"FallGuys_client.exe","AppName":"9c88",
            "CatalogNamespace":"0a2d9f","CatalogItemId":"abc123",
            "AppCategories":["public","games"]}"#,
    )
    .unwrap();
    fs::write(
        tmp.join("dlc.item"),
        r#"{"DisplayName":"Addon","InstallLocation":"D:\\Epic\\Addon",
            "LaunchExecutable":"a.exe","AppName":"a","CatalogNamespace":"n",
            "CatalogItemId":"c","AppCategories":["addons"]}"#,
    )
    .unwrap();
    fs::write(tmp.join("notes.txt"), "ignored").unwrap();

    let games = EpicProvider::from_dirs(tmp.clone(), PathBuf::from("launcher.exe"))
        .games()
        .unwrap();

    assert_eq!(games.len(), 1);
    assert_eq!(games[0].name, "Fall Guys");
    assert!(games[0].launch.args.contains("0a2d9f%3Aabc123%3A9c88"));
    assert!(games[0].launch.program.ends_with("explorer.exe"));

    fs::remove_dir_all(&tmp).unwrap();
}
```

- [ ] **Step 6: Run the integration test**

Run: `cargo test --test epic_provider`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/ tests/ Cargo.toml Cargo.lock
git commit -m "feat: add Epic provider reading launcher manifests"
```

---

### Task 7: Icon resolution

**Files:**
- Create: `src/icon/probe.rs`, `src/icon/convert.rs`
- Modify: `src/icon/mod.rs`

**Interfaces:**
- Consumes: `platform::IconCandidate`
- Produces:
  - `icon::probe::has_icon(path: &Path) -> bool`
  - `icon::convert::png_to_ico(src: &Path, dest: &Path) -> Result<(), IconError>`
  - `icon::ResolvedIcon { path: PathBuf, index: i32 }`
  - `icon::resolve(candidates: &[IconCandidate], cache_dir: &Path, key: &str) -> Option<ResolvedIcon>`

- [ ] **Step 1: Write the icon probe**

Rather than hand-parsing the PE resource directory, ask Windows the same
question it asks itself. `ExtractIconExW` with null output buffers returns the
number of icons in a file.

Create `src/icon/probe.rs`:

```rust
use std::path::Path;

/// True when Windows can draw an icon out of this file. An executable with no
/// icon resource must fall through to the next candidate rather than produce
/// a blank shortcut.
#[cfg(windows)]
pub fn has_icon(path: &Path) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ExtractIconExW;

    if !path.is_file() {
        return false;
    }
    let wide = HSTRING::from(path.as_os_str());
    let count = unsafe { ExtractIconExW(&wide, -1, None, None, 0) };
    count > 0
}

#[cfg(not(windows))]
pub fn has_icon(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_have_no_icon() {
        assert!(!has_icon(Path::new(r"C:\definitely\not\here.exe")));
    }

    #[cfg(windows)]
    #[test]
    fn a_system_executable_has_an_icon() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(has_icon(Path::new(&windir).join("explorer.exe").as_path()));
    }

    #[cfg(windows)]
    #[test]
    fn a_text_file_has_no_icon() {
        let p = std::env::temp_dir().join("wso_probe_no_icon.txt");
        std::fs::write(&p, b"hello").unwrap();
        assert!(!has_icon(&p));
        std::fs::remove_file(&p).ok();
    }
}
```

Extend the `windows` features:

```bash
cargo add windows --features Win32_System_Registry,Win32_Foundation,Win32_UI_Shell
```

- [ ] **Step 2: Run the probe tests**

Run: `cargo test probe`
Expected: PASS, 3 tests

- [ ] **Step 3: Write the failing conversion test**

Create `src/icon/convert.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_png(path: &std::path::Path, size: u32) {
        let mut img = image::RgbaImage::new(size, size);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
        img.save(path).unwrap();
    }

    #[test]
    fn converts_a_png_into_a_multi_size_ico() {
        let dir = std::env::temp_dir().join("wso_convert_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("logo.png");
        let dest = dir.join("out.ico");
        write_test_png(&src, 300);

        png_to_ico(&src, &dest).unwrap();

        assert!(dest.is_file());
        assert!(std::fs::metadata(&dest).unwrap().len() > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reports_a_missing_source() {
        let dest = std::env::temp_dir().join("wso_convert_missing.ico");
        assert!(matches!(
            png_to_ico(std::path::Path::new("nope.png"), &dest),
            Err(IconError::Decode(_))
        ));
    }
}
```

Add dependencies:

```bash
cargo add image --no-default-features --features png,jpeg
cargo add ico
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test convert`
Expected: FAIL — `cannot find function 'png_to_ico' in this scope`

- [ ] **Step 5: Write the conversion implementation**

Prepend to `src/icon/convert.rs`:

```rust
use std::path::Path;

#[derive(Debug)]
pub enum IconError {
    Decode(String),
    Write(String),
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconError::Decode(d) => write!(f, "could not decode image: {d}"),
            IconError::Write(d) => write!(f, "could not write icon: {d}"),
        }
    }
}

impl std::error::Error for IconError {}

/// Sizes Windows asks for across the Start Menu, the desktop and Alt-Tab.
const SIZES: [u32; 4] = [256, 48, 32, 16];

/// Converts a raster image into a multi-resolution `.ico`. Used only as the
/// fallback when a game's executable carries no usable icon.
pub fn png_to_ico(src: &Path, dest: &Path) -> Result<(), IconError> {
    let img = image::open(src).map_err(|e| IconError::Decode(e.to_string()))?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        // `resize` preserves aspect ratio, so pad onto a square canvas to keep
        // non-square logos from being distorted.
        let scaled = img.resize(size, size, image::imageops::FilterType::Lanczos3);
        let mut canvas = image::RgbaImage::new(size, size);
        let sx = (size - scaled.width()) / 2;
        let sy = (size - scaled.height()) / 2;
        image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), sx as i64, sy as i64);

        let entry = ico::IconImage::from_rgba_data(size, size, canvas.into_raw());
        dir.add_entry(
            ico::IconDirEntry::encode(&entry).map_err(|e| IconError::Write(e.to_string()))?,
        );
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IconError::Write(e.to_string()))?;
    }
    let file = std::fs::File::create(dest).map_err(|e| IconError::Write(e.to_string()))?;
    dir.write(file).map_err(|e| IconError::Write(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test convert`
Expected: PASS, 2 tests

- [ ] **Step 7: Write the failing resolver test**

Replace `src/icon/mod.rs` with:

```rust
pub mod convert;
pub mod exe_finder;
pub mod probe;

use crate::platform::IconCandidate;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIcon {
    pub path: PathBuf,
    pub index: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_a_missing_executable_and_takes_the_fallback() {
        let dir = std::env::temp_dir().join("wso_resolve_test_a");
        std::fs::create_dir_all(&dir).unwrap();
        let fallback = dir.join("fallback.exe");
        std::fs::write(&fallback, b"x").unwrap();

        let candidates = vec![
            IconCandidate::Executable(dir.join("missing.exe")),
            IconCandidate::Fallback(fallback.clone()),
        ];
        let resolved = resolve(&candidates, &dir, "123").unwrap();

        assert_eq!(resolved.path, fallback);
        assert_eq!(resolved.index, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn converts_an_image_candidate_into_the_cache() {
        let dir = std::env::temp_dir().join("wso_resolve_test_b");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("logo.png");
        image::RgbaImage::new(64, 64).save(&png).unwrap();

        let candidates = vec![
            IconCandidate::Executable(dir.join("missing.exe")),
            IconCandidate::ImageFile(png),
        ];
        let resolved = resolve(&candidates, &cache, "440").unwrap();

        assert_eq!(resolved.path, cache.join("440.ico"));
        assert!(resolved.path.is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn returns_none_when_nothing_resolves() {
        let dir = std::env::temp_dir().join("wso_resolve_test_c");
        std::fs::create_dir_all(&dir).unwrap();
        let candidates = vec![IconCandidate::Executable(dir.join("missing.exe"))];
        assert!(resolve(&candidates, &dir, "1").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 8: Run test to verify it fails**

Run: `cargo test icon::tests`
Expected: FAIL — `cannot find function 'resolve' in this scope`

- [ ] **Step 9: Write the resolver**

Append to `src/icon/mod.rs`:

```rust
/// Walks a game's ordered icon candidates and returns the first that yields
/// something Windows can actually draw.
///
/// `cache_dir` receives generated `.ico` files, named by `key` so repeated
/// runs reuse them instead of re-encoding.
pub fn resolve(
    candidates: &[IconCandidate],
    cache_dir: &Path,
    key: &str,
) -> Option<ResolvedIcon> {
    for candidate in candidates {
        match candidate {
            IconCandidate::Executable(path) => {
                if probe::has_icon(path) {
                    return Some(ResolvedIcon { path: path.clone(), index: 0 });
                }
            }
            IconCandidate::ImageFile(src) => {
                if !src.is_file() {
                    continue;
                }
                let dest = cache_dir.join(format!("{key}.ico"));
                if dest.is_file() {
                    return Some(ResolvedIcon { path: dest, index: 0 });
                }
                if convert::png_to_ico(src, &dest).is_ok() {
                    return Some(ResolvedIcon { path: dest, index: 0 });
                }
            }
            IconCandidate::Fallback(path) => {
                if path.is_file() {
                    return Some(ResolvedIcon { path: path.clone(), index: 0 });
                }
            }
        }
    }
    None
}
```

- [ ] **Step 10: Run the full suite**

Run: `cargo test`
Expected: PASS, 45 tests

- [ ] **Step 11: Commit**

```bash
git add src/ Cargo.toml Cargo.lock
git commit -m "feat: add icon resolution with executable probe and PNG fallback"
```

---

### Task 8: Shortcut reading and writing

**Files:**
- Create: `src/shortcut/writer.rs`, `tests/lnk_roundtrip.rs`
- Modify: `src/shortcut/mod.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `shortcut::LnkData { target: PathBuf, args: String, working_dir: Option<PathBuf>, icon: Option<(PathBuf, i32)>, description: String }`
  - `shortcut::writer::ComGuard::new() -> Result<ComGuard, ShortcutError>`
  - `shortcut::writer::write_lnk(path: &Path, data: &LnkData) -> Result<(), ShortcutError>`
  - `shortcut::writer::read_lnk(path: &Path) -> Result<LnkData, ShortcutError>`

- [ ] **Step 1: Define the model**

Replace `src/shortcut/mod.rs` with:

```rust
pub mod signature;
pub mod writer;

use std::path::PathBuf;

/// The subset of a Windows shortcut this tool reads and writes. `icon` is
/// deliberately independent of `target`: that separation is what lets a
/// shortcut launch through Steam while showing the game's own icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LnkData {
    pub target: PathBuf,
    pub args: String,
    pub working_dir: Option<PathBuf>,
    pub icon: Option<(PathBuf, i32)>,
    pub description: String,
}
```

- [ ] **Step 2: Write the failing round-trip test**

Create `tests/lnk_roundtrip.rs`:

```rust
#![cfg(windows)]

use std::path::PathBuf;
use winshortcutsorganizer::shortcut::{
    writer::{read_lnk, write_lnk, ComGuard},
    LnkData,
};

#[test]
fn writes_and_reads_back_every_field() {
    let _com = ComGuard::new().unwrap();
    let dir = std::env::temp_dir().join("wso_lnk_roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Terraria.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");

    let data = LnkData {
        target: explorer.clone(),
        args: "-applaunch 105600".into(),
        working_dir: Some(PathBuf::from(&windir)),
        icon: Some((explorer.clone(), 0)),
        description: "Terraria — managed by WinShortcutsOrganizer [wso:steam:105600]".into(),
    };

    write_lnk(&lnk, &data).unwrap();
    assert!(lnk.is_file());

    let read = read_lnk(&lnk).unwrap();
    assert_eq!(read.target, explorer);
    assert_eq!(read.args, "-applaunch 105600");
    assert_eq!(read.icon, Some((explorer, 0)));
    assert_eq!(read.description, data.description);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preserves_a_uri_argument_for_epic() {
    let _com = ComGuard::new().unwrap();
    let dir = std::env::temp_dir().join("wso_lnk_uri");
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Fall Guys.lnk");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let explorer = PathBuf::from(&windir).join("explorer.exe");
    let uri = "\"com.epicgames.launcher://apps/a%3Ab%3Ac?action=launch&silent=true\"";

    write_lnk(
        &lnk,
        &LnkData {
            target: explorer,
            args: uri.into(),
            working_dir: None,
            icon: None,
            description: "Fall Guys — managed by WinShortcutsOrganizer [wso:epic:c]".into(),
        },
    )
    .unwrap();

    assert_eq!(read_lnk(&lnk).unwrap().args, uri);
    std::fs::remove_dir_all(&dir).ok();
}
```

Extend the `windows` features:

```bash
cargo add windows --features Win32_System_Registry,Win32_Foundation,Win32_UI_Shell,Win32_System_Com,Win32_UI_Shell_Common,Win32_Storage_FileSystem
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test lnk_roundtrip`
Expected: FAIL — `unresolved import 'winshortcutsorganizer::shortcut::writer'`

- [ ] **Step 4: Write the COM implementation**

Create `src/shortcut/writer.rs`:

```rust
use super::LnkData;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ShortcutError {
    Com(String),
    Io(String),
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShortcutError::Com(d) => write!(f, "shell link error: {d}"),
            ShortcutError::Io(d) => write!(f, "file error: {d}"),
        }
    }
}

impl std::error::Error for ShortcutError {}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    /// COM must be initialized on every thread that touches IShellLink, and
    /// uninitialized when that thread is done. Tests create their own.
    pub struct ComGuard;

    impl ComGuard {
        pub fn new() -> Result<ComGuard, ShortcutError> {
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|e| ShortcutError::Com(e.to_string()))?;
            }
            Ok(ComGuard)
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn buf_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    pub fn write_lnk(path: &Path, data: &LnkData) -> Result<(), ShortcutError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ShortcutError::Io(e.to_string()))?;
        }
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            link.SetPath(&HSTRING::from(data.target.as_os_str()))
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
            link.SetArguments(&HSTRING::from(data.args.as_str()))
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
            link.SetDescription(&HSTRING::from(data.description.as_str()))
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            if let Some(wd) = &data.working_dir {
                link.SetWorkingDirectory(&HSTRING::from(wd.as_os_str()))
                    .map_err(|e| ShortcutError::Com(e.to_string()))?;
            }
            if let Some((icon_path, index)) = &data.icon {
                link.SetIconLocation(&HSTRING::from(icon_path.as_os_str()), *index)
                    .map_err(|e| ShortcutError::Com(e.to_string()))?;
            }

            let persist: IPersistFile =
                link.cast().map_err(|e| ShortcutError::Com(e.to_string()))?;
            persist
                .Save(&HSTRING::from(path.as_os_str()), true)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
        }
        Ok(())
    }

    pub fn read_lnk(path: &Path) -> Result<LnkData, ShortcutError> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;
            let persist: IPersistFile =
                link.cast().map_err(|e| ShortcutError::Com(e.to_string()))?;
            persist
                .Load(&HSTRING::from(path.as_os_str()), 0)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut target = [0u16; 260];
            link.GetPath(&mut target, std::ptr::null_mut(), 0)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut args = [0u16; 1024];
            link.GetArguments(&mut args)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut desc = [0u16; 1024];
            link.GetDescription(&mut desc)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut wd = [0u16; 260];
            link.GetWorkingDirectory(&mut wd)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let mut icon = [0u16; 260];
            let mut index = 0i32;
            link.GetIconLocation(&mut icon, &mut index)
                .map_err(|e| ShortcutError::Com(e.to_string()))?;

            let icon_str = buf_to_string(&icon);
            let wd_str = buf_to_string(&wd);

            Ok(LnkData {
                target: PathBuf::from(buf_to_string(&target)),
                args: buf_to_string(&args),
                working_dir: if wd_str.is_empty() { None } else { Some(PathBuf::from(wd_str)) },
                icon: if icon_str.is_empty() {
                    None
                } else {
                    Some((PathBuf::from(icon_str), index))
                },
                description: buf_to_string(&desc),
            })
        }
    }

    // Silence an unused-import warning on the PCWSTR alias when features change.
    #[allow(dead_code)]
    fn _assert_pcwstr(_: PCWSTR) {}
}

#[cfg(windows)]
pub use imp::{read_lnk, write_lnk, ComGuard};

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct ComGuard;

    impl ComGuard {
        pub fn new() -> Result<ComGuard, ShortcutError> {
            Err(ShortcutError::Com("shortcuts require Windows".into()))
        }
    }

    pub fn write_lnk(_path: &Path, _data: &LnkData) -> Result<(), ShortcutError> {
        Err(ShortcutError::Com("shortcuts require Windows".into()))
    }

    pub fn read_lnk(_path: &Path) -> Result<LnkData, ShortcutError> {
        Err(ShortcutError::Com("shortcuts require Windows".into()))
    }
}

#[cfg(not(windows))]
pub use imp::{read_lnk, write_lnk, ComGuard};
```

- [ ] **Step 5: Run the round-trip test**

Run: `cargo test --test lnk_roundtrip`
Expected: PASS, 2 tests

If `GetPath` returns an empty string, the target file did not exist when the
link was saved — Windows resolves and validates the path on save. Point the
test at a real system executable, as written above.

- [ ] **Step 6: Commit**

```bash
git add src/ tests/ Cargo.toml Cargo.lock
git commit -m "feat: read and write Windows shortcuts through IShellLink"
```

---

### Task 9: Target folders and organization modes

**Files:**
- Modify: `src/layout.rs`

**Interfaces:**
- Consumes: `layout::sanitize_filename`, `platform::Platform`
- Produces:
  - `layout::Target` enum: `Desktop`, `StartMenu`
  - `layout::Organization` enum: `Flat`, `Unified { folder: String }`, `PerPlatform { parent: Option<String> }` — struct variants throughout, because Task 11 serializes this to TOML and TOML cannot represent a newtype variant under an internal tag
  - `layout::target_dir(target: Target) -> Result<PathBuf, LayoutError>`
  - `layout::shortcut_dir(base: &Path, org: &Organization, platform: Platform) -> PathBuf`
  - `layout::assign_filenames(entries: &[(Platform, String)]) -> Vec<String>` — resolves collisions
  - `layout::managed_dirs(base: &Path, org: &Organization) -> Vec<PathBuf>` — every directory the tool may own under a base

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `src/layout.rs`:

```rust
    #[test]
    fn flat_mode_uses_the_base_directory() {
        let base = PathBuf::from(r"C:\Desktop");
        assert_eq!(
            shortcut_dir(&base, &Organization::Flat, Platform::Steam),
            base
        );
    }

    #[test]
    fn unified_mode_uses_one_folder_for_every_platform() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::Unified { folder: "Games".into() };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("Games")
        );
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Epic),
            base.join("Games")
        );
    }

    #[test]
    fn per_platform_mode_splits_by_platform() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::PerPlatform { parent: None };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("Steam")
        );

        let nested = Organization::PerPlatform { parent: Some("Games".into()) };
        assert_eq!(
            shortcut_dir(&base, &nested, Platform::Epic),
            base.join("Games").join("Epic")
        );
    }

    #[test]
    fn folder_names_are_sanitized_too() {
        let base = PathBuf::from(r"C:\Desktop");
        let org = Organization::Unified { folder: "My: Games".into() };
        assert_eq!(
            shortcut_dir(&base, &org, Platform::Steam),
            base.join("My Games")
        );
    }

    #[test]
    fn distinct_names_are_left_alone() {
        let entries = [
            (Platform::Steam, "Terraria".to_string()),
            (Platform::Epic, "Fall Guys".to_string()),
        ];
        assert_eq!(assign_filenames(&entries), vec!["Terraria", "Fall Guys"]);
    }

    #[test]
    fn cross_platform_collisions_get_a_platform_suffix() {
        let entries = [
            (Platform::Steam, "Fallout 4".to_string()),
            (Platform::Epic, "Fallout 4".to_string()),
        ];
        assert_eq!(
            assign_filenames(&entries),
            vec!["Fallout 4 (Steam)", "Fallout 4 (Epic)"]
        );
    }

    #[test]
    fn same_platform_collisions_get_a_numeric_suffix() {
        let entries = [
            (Platform::Steam, "Portal".to_string()),
            (Platform::Steam, "Portal".to_string()),
            (Platform::Steam, "Portal".to_string()),
        ];
        assert_eq!(
            assign_filenames(&entries),
            vec!["Portal (Steam)", "Portal (Steam) 2", "Portal (Steam) 3"]
        );
    }

    #[test]
    fn collisions_are_detected_after_sanitization() {
        let entries = [
            (Platform::Steam, "X4: Foundations".to_string()),
            (Platform::Epic, "X4 Foundations".to_string()),
        ];
        assert_eq!(
            assign_filenames(&entries),
            vec!["X4 Foundations (Steam)", "X4 Foundations (Epic)"]
        );
    }

    #[test]
    fn managed_dirs_lists_every_directory_we_may_own() {
        let base = PathBuf::from(r"C:\Desktop");
        assert_eq!(managed_dirs(&base, &Organization::Flat), vec![base.clone()]);
        assert_eq!(
            managed_dirs(&base, &Organization::Unified { folder: "Games".into() }),
            vec![base.join("Games")]
        );
        assert_eq!(
            managed_dirs(&base, &Organization::PerPlatform { parent: Some("Games".into()) }),
            vec![base.join("Games").join("Steam"), base.join("Games").join("Epic")]
        );
    }
```

Add the imports the tests need at the top of the `tests` module:

```rust
    use crate::platform::Platform;
    use std::path::PathBuf;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test layout`
Expected: FAIL — `cannot find function 'shortcut_dir' in this scope`

- [ ] **Step 3: Write minimal implementation**

Append to `src/layout.rs` (above the tests module):

```rust
use crate::platform::Platform;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Desktop,
    StartMenu,
}

impl Target {
    pub fn label(&self) -> &'static str {
        match self {
            Target::Desktop => "Desktop",
            Target::StartMenu => "Start Menu",
        }
    }
}

/// Struct variants throughout, not newtype variants: Task 11 serializes this
/// to TOML under an internal tag, which newtype variants cannot represent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", tag = "mode")]
pub enum Organization {
    /// Shortcuts sit directly in the target directory.
    Flat,
    /// One folder holds every platform's shortcuts.
    Unified { folder: String },
    /// One folder per platform, optionally nested under a shared parent.
    PerPlatform { parent: Option<String> },
}

#[derive(Debug)]
pub enum LayoutError {
    UnknownFolder(String),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::UnknownFolder(d) => {
                write!(f, "could not locate a Windows known folder: {d}")
            }
        }
    }
}

impl std::error::Error for LayoutError {}

pub fn shortcut_dir(base: &Path, org: &Organization, platform: Platform) -> PathBuf {
    match org {
        Organization::Flat => base.to_path_buf(),
        Organization::Unified { folder } => base.join(sanitize_filename(folder)),
        Organization::PerPlatform { parent } => {
            let root = match parent {
                Some(p) => base.join(sanitize_filename(p)),
                None => base.to_path_buf(),
            };
            root.join(sanitize_filename(platform.display_name()))
        }
    }
}

/// Every directory the tool may place shortcuts into under `base`. Used to
/// know where to scan for shortcuts we already own.
pub fn managed_dirs(base: &Path, org: &Organization) -> Vec<PathBuf> {
    match org {
        Organization::Flat | Organization::Unified { .. } => {
            vec![shortcut_dir(base, org, Platform::Steam)]
        }
        Organization::PerPlatform { .. } => vec![
            shortcut_dir(base, org, Platform::Steam),
            shortcut_dir(base, org, Platform::Epic),
        ],
    }
}

/// Produces a unique, filesystem-safe basename for each entry, in input order.
///
/// Two games can legitimately share a title across platforms, and sanitization
/// can turn two different titles into the same string, so uniqueness is
/// decided after sanitization rather than before.
pub fn assign_filenames(entries: &[(Platform, String)]) -> Vec<String> {
    let sanitized: Vec<String> = entries
        .iter()
        .map(|(_, name)| sanitize_filename(name))
        .collect();

    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for name in &sanitized {
        *counts.entry(name.as_str()).or_insert(0) += 1;
    }

    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(entries.len());

    for (i, base) in sanitized.iter().enumerate() {
        let mut candidate = if counts[base.as_str()] > 1 {
            format!("{base} ({})", entries[i].0.display_name())
        } else {
            base.clone()
        };

        let mut n = 2;
        while used.contains(&candidate) {
            candidate = format!("{base} ({}) {n}", entries[i].0.display_name());
            n += 1;
        }

        used.insert(candidate.clone());
        out.push(candidate);
    }
    out
}
```

Add the known-folder lookup at the end of `src/layout.rs`:

```rust
#[cfg(windows)]
pub fn target_dir(target: Target) -> Result<PathBuf, LayoutError> {
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Programs, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
    };

    // FOLDERID_Programs is the per-user Start Menu. FOLDERID_CommonPrograms
    // is deliberately never used: writing there would require elevation.
    let id = match target {
        Target::Desktop => FOLDERID_Desktop,
        Target::StartMenu => FOLDERID_Programs,
    };

    unsafe {
        let pwstr = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None)
            .map_err(|e| LayoutError::UnknownFolder(e.to_string()))?;
        let s = pwstr
            .to_string()
            .map_err(|e| LayoutError::UnknownFolder(e.to_string()))?;
        windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.0 as *const _));
        Ok(PathBuf::from(s))
    }
}

#[cfg(not(windows))]
pub fn target_dir(_target: Target) -> Result<PathBuf, LayoutError> {
    Err(LayoutError::UnknownFolder("Windows only".into()))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 54 tests

- [ ] **Step 5: Verify the known folders resolve on this machine**

Add a Windows-only test at the end of the `tests` module:

```rust
    #[cfg(windows)]
    #[test]
    fn resolves_per_user_known_folders() {
        let desktop = target_dir(Target::Desktop).unwrap();
        let start = target_dir(Target::StartMenu).unwrap();
        assert!(desktop.is_dir(), "desktop should exist: {}", desktop.display());
        assert!(start.is_dir(), "start menu should exist: {}", start.display());
        assert!(
            start.to_string_lossy().contains("AppData"),
            "must be the per-user Start Menu, got {}",
            start.display()
        );
    }
```

Run: `cargo test layout`
Expected: PASS. The `AppData` assertion is the guard that we never resolved
the all-users Start Menu.

- [ ] **Step 6: Commit**

```bash
git add src/
git commit -m "feat: add target folders, organization modes and collision handling"
```

---

### Task 10: Pure reconciliation

This is the heart of the tool. Every command is this one function with
different inputs, so it carries the heaviest test burden.

**Files:**
- Modify: `src/plan.rs`

**Interfaces:**
- Consumes: `shortcut::LnkData`, `shortcut::signature::Signature`
- Produces:
  - `plan::DesiredShortcut { sig: Signature, path: PathBuf, lnk: LnkData }`
  - `plan::OwnedShortcut { sig: Signature, path: PathBuf, lnk: LnkData }`
  - `plan::SkipReason` enum: `UpToDate`
  - `plan::Action` enum: `Create(DesiredShortcut)`, `Retarget { path, lnk }`, `FixIcon { path, lnk }`, `Move { from, to, lnk }`, `DeleteOrphan(PathBuf)`, `Skip { path: PathBuf, reason: SkipReason }`
  - `plan::plan(desired: &[DesiredShortcut], owned: &[OwnedShortcut]) -> Vec<Action>`

- [ ] **Step 1: Write the failing test**

Replace `src/plan.rs` with:

```rust
//! Pure reconciliation between the shortcuts that should exist and the
//! shortcuts this tool already owns. No I/O happens here, which is what makes
//! every command's behavior testable without Windows, Steam or Epic.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use std::path::PathBuf;

    fn sig(id: &str) -> Signature {
        Signature { platform: Platform::Steam, id: id.into() }
    }

    fn lnk(target: &str, args: &str, icon: &str) -> LnkData {
        LnkData {
            target: PathBuf::from(target),
            args: args.into(),
            working_dir: None,
            icon: Some((PathBuf::from(icon), 0)),
            description: "d".into(),
        }
    }

    fn desired(id: &str, path: &str, icon: &str) -> DesiredShortcut {
        DesiredShortcut {
            sig: sig(id),
            path: PathBuf::from(path),
            lnk: lnk("steam.exe", &format!("-applaunch {id}"), icon),
        }
    }

    fn owned(id: &str, path: &str, icon: &str) -> OwnedShortcut {
        OwnedShortcut {
            sig: sig(id),
            path: PathBuf::from(path),
            lnk: lnk("steam.exe", &format!("-applaunch {id}"), icon),
        }
    }

    #[test]
    fn creates_a_missing_shortcut() {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let actions = plan(&d, &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Create(_)));
    }

    #[test]
    fn skips_an_up_to_date_shortcut() {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(
            plan(&d, &o).as_slice(),
            [Action::Skip { reason: SkipReason::UpToDate, .. }]
        ));
    }

    #[test]
    fn is_idempotent() {
        let d = [desired("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        let first = plan(&d, &o);
        let second = plan(&d, &o);
        assert_eq!(first.len(), second.len());
        assert!(first.iter().all(|a| matches!(a, Action::Skip { .. })));
    }

    #[test]
    fn fixes_a_changed_icon() {
        let d = [desired("440", r"C:\D\TF2.lnk", "new.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "old.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::FixIcon { .. }]));
    }

    #[test]
    fn retargets_when_the_launch_command_changed() {
        let d = [DesiredShortcut {
            sig: sig("440"),
            path: PathBuf::from(r"C:\D\TF2.lnk"),
            lnk: lnk(r"D:\Steam\steam.exe", "-applaunch 440", "tf2.exe"),
        }];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Retarget { .. }]));
    }

    #[test]
    fn moves_when_the_organization_mode_changed() {
        let d = [desired("440", r"C:\D\Games\TF2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        match plan(&d, &o).as_slice() {
            [Action::Move { from, to, .. }] => {
                assert_eq!(from, &PathBuf::from(r"C:\D\TF2.lnk"));
                assert_eq!(to, &PathBuf::from(r"C:\D\Games\TF2.lnk"));
            }
            other => panic!("expected a Move, got {other:?}"),
        }
    }

    #[test]
    fn a_move_wins_over_an_icon_change() {
        // A relocated shortcut is rewritten wholesale at the new path, so
        // there is no separate FixIcon to emit.
        let d = [desired("440", r"C:\D\Games\TF2.lnk", "new.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "old.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn deletes_an_orphan_when_the_game_is_gone() {
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(
            plan(&[], &o).as_slice(),
            [Action::DeleteOrphan(_)]
        ));
    }

    #[test]
    fn clean_deletes_everything_owned() {
        let o = [
            owned("440", r"C:\D\TF2.lnk", "a.exe"),
            owned("570", r"C:\D\Dota.lnk", "b.exe"),
        ];
        let actions = plan(&[], &o);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|a| matches!(a, Action::DeleteOrphan(_))));
    }

    #[test]
    fn matches_on_signature_not_on_path() {
        // The same game, renamed on disk by the user, is still the same game.
        let d = [desired("440", r"C:\D\Team Fortress 2.lnk", "tf2.exe")];
        let o = [owned("440", r"C:\D\TF2.lnk", "tf2.exe")];
        assert!(matches!(plan(&d, &o).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn different_platforms_with_the_same_id_do_not_match() {
        let d = [DesiredShortcut {
            sig: Signature { platform: Platform::Epic, id: "440".into() },
            path: PathBuf::from(r"C:\D\E.lnk"),
            lnk: lnk("explorer.exe", "uri", "e.exe"),
        }];
        let o = [owned("440", r"C:\D\S.lnk", "s.exe")];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().any(|a| matches!(a, Action::Create(_))));
        assert!(actions.iter().any(|a| matches!(a, Action::DeleteOrphan(_))));
    }

    #[test]
    fn handles_a_mixed_batch() {
        let d = [
            desired("440", r"C:\D\TF2.lnk", "tf2.exe"),   // up to date
            desired("570", r"C:\D\Dota.lnk", "new.exe"),  // icon changed
            desired("620", r"C:\D\P2.lnk", "p2.exe"),     // new
        ];
        let o = [
            owned("440", r"C:\D\TF2.lnk", "tf2.exe"),
            owned("570", r"C:\D\Dota.lnk", "old.exe"),
            owned("999", r"C:\D\Gone.lnk", "gone.exe"),   // uninstalled
        ];
        let actions = plan(&d, &o);
        assert_eq!(actions.len(), 4);
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::Skip { .. })).count(),
            1
        );
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::FixIcon { .. })).count(),
            1
        );
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::Create(_))).count(),
            1
        );
        assert_eq!(
            actions.iter().filter(|a| matches!(a, Action::DeleteOrphan(_))).count(),
            1
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test plan`
Expected: FAIL — `cannot find type 'DesiredShortcut' in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/plan.rs` (below the module doc comment):

```rust
use crate::shortcut::signature::Signature;
use crate::shortcut::LnkData;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredShortcut {
    pub sig: Signature,
    pub path: PathBuf,
    pub lnk: LnkData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedShortcut {
    pub sig: Signature,
    pub path: PathBuf,
    pub lnk: LnkData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    UpToDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create(DesiredShortcut),
    Retarget { path: PathBuf, lnk: LnkData },
    FixIcon { path: PathBuf, lnk: LnkData },
    Move { from: PathBuf, to: PathBuf, lnk: LnkData },
    DeleteOrphan(PathBuf),
    Skip { path: PathBuf, reason: SkipReason },
}

/// Reconciles what should exist against what this tool already owns.
///
/// Matching is by signature, never by path or filename, so a shortcut the
/// user renamed or moved is still recognized as the same game.
pub fn plan(desired: &[DesiredShortcut], owned: &[OwnedShortcut]) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut matched: Vec<bool> = vec![false; owned.len()];

    for d in desired {
        let existing = owned.iter().position(|o| o.sig == d.sig);

        let Some(idx) = existing else {
            actions.push(Action::Create(d.clone()));
            continue;
        };
        matched[idx] = true;
        let o = &owned[idx];

        // A relocation rewrites the file at the new path, so it subsumes any
        // target or icon change and must be checked first.
        if o.path != d.path {
            actions.push(Action::Move {
                from: o.path.clone(),
                to: d.path.clone(),
                lnk: d.lnk.clone(),
            });
        } else if o.lnk.target != d.lnk.target
            || o.lnk.args != d.lnk.args
            || o.lnk.working_dir != d.lnk.working_dir
        {
            actions.push(Action::Retarget { path: d.path.clone(), lnk: d.lnk.clone() });
        } else if o.lnk.icon != d.lnk.icon {
            actions.push(Action::FixIcon { path: d.path.clone(), lnk: d.lnk.clone() });
        } else {
            actions.push(Action::Skip {
                path: d.path.clone(),
                reason: SkipReason::UpToDate,
            });
        }
    }

    for (idx, o) in owned.iter().enumerate() {
        if !matched[idx] {
            actions.push(Action::DeleteOrphan(o.path.clone()));
        }
    }

    actions
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 66 tests

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "feat: add pure reconciliation between desired and owned shortcuts"
```

---

### Task 11: Configuration file

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `layout::{Organization, Target}`, `platform::Platform`
- Produces:
  - `config::Config { targets: Vec<Target>, organization: Organization, platforms: Vec<Platform>, exclusions: Vec<String>, icon_overrides: HashMap<String, PathBuf> }`
  - `config::Config::default() -> Config`
  - `config::config_path() -> PathBuf` — `%APPDATA%\WinShortcutsOrganizer\config.toml`
  - `config::cache_dir() -> PathBuf` — `%LOCALAPPDATA%\WinShortcutsOrganizer\icons`
  - `config::load(path: &Path) -> Config` — missing or unreadable file yields defaults
  - `config::save(path: &Path, cfg: &Config) -> Result<(), ConfigError>`
  - `Config::is_excluded(&self, platform: Platform, id: &str, name: &str) -> bool`

- [ ] **Step 1: Write the failing test**

Create `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let c = Config::default();
        assert_eq!(c.targets, vec![Target::StartMenu]);
        assert_eq!(c.organization, Organization::Unified { folder: "Games".into() });
        assert_eq!(c.platforms, vec![Platform::Steam, Platform::Epic]);
        assert!(c.exclusions.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let dir = std::env::temp_dir().join("wso_config_roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut c = Config::default();
        c.targets = vec![Target::Desktop, Target::StartMenu];
        c.organization = Organization::PerPlatform { parent: Some("Games".into()) };
        c.exclusions = vec!["steam:228980".into(), "Wallpaper Engine".into()];
        c.icon_overrides
            .insert("steam:440".into(), PathBuf::from(r"C:\icons\tf2.ico"));

        save(&path, &c).unwrap();
        assert_eq!(load(&path), c);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        assert_eq!(load(Path::new(r"C:\nope\config.toml")), Config::default());
    }

    #[test]
    fn a_corrupt_file_yields_defaults() {
        let dir = std::env::temp_dir().join("wso_config_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        assert_eq!(load(&path), Config::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exclusions_match_by_qualified_id_or_by_name() {
        let mut c = Config::default();
        c.exclusions = vec!["steam:228980".into(), "Wallpaper Engine".into()];

        assert!(c.is_excluded(Platform::Steam, "228980", "Redistributables"));
        assert!(c.is_excluded(Platform::Steam, "431960", "Wallpaper Engine"));
        assert!(c.is_excluded(Platform::Steam, "431960", "wallpaper engine"));
        assert!(!c.is_excluded(Platform::Epic, "228980", "Something Else"));
        assert!(!c.is_excluded(Platform::Steam, "105600", "Terraria"));
    }
}
```

Add `pub mod config;` to `src/lib.rs`, then:

```bash
cargo add toml
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test config`
Expected: FAIL — `cannot find type 'Config' in this scope`

- [ ] **Step 3: Write minimal implementation**

Prepend to `src/config.rs`:

```rust
use crate::layout::{Organization, Target};
use crate::platform::Platform;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Serialize(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(d) => write!(f, "could not write config: {d}"),
            ConfigError::Serialize(d) => write!(f, "could not serialize config: {d}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Persisted user choices. This file holds preferences only — never ownership.
/// Which shortcuts belong to the tool is recorded in the shortcuts themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub targets: Vec<Target>,
    pub organization: Organization,
    pub platforms: Vec<Platform>,
    /// Either a qualified id (`steam:228980`) or a case-insensitive game name.
    pub exclusions: Vec<String>,
    /// Qualified id to an icon file, for the cases the heuristic gets wrong.
    pub icon_overrides: HashMap<String, PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            targets: vec![Target::StartMenu],
            organization: Organization::Unified { folder: "Games".into() },
            platforms: vec![Platform::Steam, Platform::Epic],
            exclusions: Vec::new(),
            icon_overrides: HashMap::new(),
        }
    }
}

impl Config {
    pub fn is_excluded(&self, platform: Platform, id: &str, name: &str) -> bool {
        let qualified = format!("{}:{}", platform.as_str(), id);
        self.exclusions.iter().any(|e| {
            e.eq_ignore_ascii_case(&qualified) || e.eq_ignore_ascii_case(name)
        })
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("WinShortcutsOrganizer").join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("WinShortcutsOrganizer").join("icons")
}

/// A missing or corrupt config is not an error: the tool falls back to
/// defaults and the wizard will write a good file on the next run.
pub fn load(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(cfg).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    std::fs::write(path, text).map_err(|e| ConfigError::Io(e.to_string()))
}
```

`Target` and `Organization` already derive the serde traits from Task 9.
`Platform` does not yet, so in `src/platform/mod.rs` change its derive list to:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 71 tests. If `toml` rejects the `Organization` enum, confirm
the `tag = "mode"` attribute from Task 9 is present and that no variant was
turned back into a newtype.

- [ ] **Step 5: Commit**

```bash
git add src/ Cargo.toml Cargo.lock
git commit -m "feat: add persisted configuration with exclusions and icon overrides"
```

---

### Task 12: Scanning and applying

**Files:**
- Create: `src/ops.rs`, `tests/ops_apply.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `plan::{plan, Action, DesiredShortcut, OwnedShortcut}`, `shortcut::writer`, `shortcut::signature`, `layout`, `icon`, `config::Config`, `platform::Game`
- Produces:
  - `ops::scan_owned(dirs: &[PathBuf]) -> Vec<OwnedShortcut>`
  - `ops::build_desired(games: &[Game], cfg: &Config, base: &Path, cache: &Path) -> Vec<DesiredShortcut>`
  - `ops::Outcome { action: String, name: String, result: Result<(), String> }`
  - `ops::apply(actions: &[Action], dry_run: bool) -> Vec<Outcome>`
  - `ops::Summary { created, updated, moved, deleted, skipped, failed: usize }`
  - `ops::summarize(outcomes: &[Outcome]) -> Summary`

- [ ] **Step 1: Write the failing test for scanning**

Create `tests/ops_apply.rs`:

```rust
#![cfg(windows)]

use std::path::PathBuf;
use winshortcutsorganizer::ops::{apply, scan_owned, summarize};
use winshortcutsorganizer::plan::Action;
use winshortcutsorganizer::shortcut::{
    signature::{encode, Signature},
    writer::{write_lnk, ComGuard},
    LnkData,
};
use winshortcutsorganizer::platform::Platform;

fn system_exe() -> PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(windir).join("explorer.exe")
}

#[test]
fn scan_claims_only_signed_shortcuts() {
    let _com = ComGuard::new().unwrap();
    let dir = std::env::temp_dir().join("wso_scan_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let sig = Signature { platform: Platform::Steam, id: "105600".into() };
    write_lnk(
        &dir.join("Terraria.lnk"),
        &LnkData {
            target: system_exe(),
            args: "-applaunch 105600".into(),
            working_dir: None,
            icon: None,
            description: encode("Terraria", &sig),
        },
    )
    .unwrap();

    // A shortcut the user made by hand. It must never be claimed.
    write_lnk(
        &dir.join("My Notes.lnk"),
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "My own shortcut".into(),
        },
    )
    .unwrap();

    std::fs::write(dir.join("readme.txt"), b"not a shortcut").unwrap();

    let owned = scan_owned(&[dir.clone()]);
    assert_eq!(owned.len(), 1, "only the signed shortcut is ours");
    assert_eq!(owned[0].sig, sig);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dry_run_changes_nothing_on_disk() {
    let _com = ComGuard::new().unwrap();
    let dir = std::env::temp_dir().join("wso_dryrun_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("Game.lnk");

    let actions = vec![Action::Create(
        winshortcutsorganizer::plan::DesiredShortcut {
            sig: Signature { platform: Platform::Steam, id: "1".into() },
            path: target.clone(),
            lnk: LnkData {
                target: system_exe(),
                args: "-applaunch 1".into(),
                working_dir: None,
                icon: None,
                description: "Game [wso:steam:1]".into(),
            },
        },
    )];

    let outcomes = apply(&actions, true);
    assert_eq!(outcomes.len(), 1);
    assert!(!target.exists(), "dry run must not write");

    let outcomes = apply(&actions, false);
    assert!(target.is_file(), "a real run must write");
    assert_eq!(summarize(&outcomes).created, 1);
    assert_eq!(summarize(&outcomes).failed, 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn delete_removes_the_file() {
    let _com = ComGuard::new().unwrap();
    let dir = std::env::temp_dir().join("wso_delete_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lnk = dir.join("Old.lnk");

    write_lnk(
        &lnk,
        &LnkData {
            target: system_exe(),
            args: String::new(),
            working_dir: None,
            icon: None,
            description: "Old [wso:steam:9]".into(),
        },
    )
    .unwrap();
    assert!(lnk.is_file());

    apply(&[Action::DeleteOrphan(lnk.clone())], false);
    assert!(!lnk.exists());

    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test ops_apply`
Expected: FAIL — `unresolved import 'winshortcutsorganizer::ops'`

- [ ] **Step 3: Write minimal implementation**

Create `src/ops.rs`:

```rust
use crate::config::Config;
use crate::icon;
use crate::layout::{self, Organization};
use crate::plan::{Action, DesiredShortcut, OwnedShortcut};
use crate::platform::Game;
use crate::shortcut::signature::{decode, encode};
use crate::shortcut::writer::{read_lnk, write_lnk};
use crate::shortcut::LnkData;
use std::path::{Path, PathBuf};

/// Reads every `.lnk` in the given directories and keeps only the ones
/// carrying our signature. Anything unsigned is invisible to the rest of the
/// tool, which is what makes deletion safe.
pub fn scan_owned(dirs: &[PathBuf]) -> Vec<OwnedShortcut> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lnk") {
                continue;
            }
            let Ok(lnk) = read_lnk(&path) else { continue };
            let Some(sig) = decode(&lnk.description) else {
                continue;
            };
            out.push(OwnedShortcut { sig, path, lnk });
        }
    }
    out
}

/// Turns detected games into the shortcuts that should exist, honoring
/// exclusions, icon overrides and the chosen organization.
pub fn build_desired(
    games: &[Game],
    cfg: &Config,
    base: &Path,
    cache: &Path,
) -> Vec<DesiredShortcut> {
    let included: Vec<&Game> = games
        .iter()
        .filter(|g| cfg.platforms.contains(&g.platform))
        .filter(|g| !cfg.is_excluded(g.platform, &g.id, &g.name))
        .collect();

    let name_inputs: Vec<(crate::platform::Platform, String)> = included
        .iter()
        .map(|g| (g.platform, g.name.clone()))
        .collect();
    let filenames = layout::assign_filenames(&name_inputs);

    included
        .iter()
        .zip(filenames)
        .map(|(g, filename)| {
            let qualified = format!("{}:{}", g.platform.as_str(), g.id);
            let resolved = cfg
                .icon_overrides
                .get(&qualified)
                .map(|p| icon::ResolvedIcon { path: p.clone(), index: 0 })
                .or_else(|| icon::resolve(&g.icon_candidates, cache, &qualified.replace(':', "_")));

            let sig = crate::shortcut::signature::Signature {
                platform: g.platform,
                id: g.id.clone(),
            };

            DesiredShortcut {
                path: layout::shortcut_dir(base, &cfg.organization, g.platform)
                    .join(format!("{filename}.lnk")),
                lnk: LnkData {
                    target: g.launch.program.clone(),
                    args: g.launch.args.clone(),
                    working_dir: g.launch.working_dir.clone(),
                    icon: resolved.map(|r| (r.path, r.index)),
                    description: encode(&g.name, &sig),
                },
                sig,
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub action: String,
    pub name: String,
    pub result: Result<(), String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub created: usize,
    pub updated: usize,
    pub moved: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub failed: usize,
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}

/// Applies actions one at a time. A failure on one shortcut is recorded and
/// the run continues: one bad game must never abort the whole operation.
pub fn apply(actions: &[Action], dry_run: bool) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(actions.len());

    for action in actions {
        let (label, name, result) = match action {
            Action::Create(d) => (
                "create",
                display_name(&d.path),
                run(dry_run, || write_lnk(&d.path, &d.lnk).map_err(|e| e.to_string())),
            ),
            Action::Retarget { path, lnk } => (
                "retarget",
                display_name(path),
                run(dry_run, || write_lnk(path, lnk).map_err(|e| e.to_string())),
            ),
            Action::FixIcon { path, lnk } => (
                "fix-icon",
                display_name(path),
                run(dry_run, || write_lnk(path, lnk).map_err(|e| e.to_string())),
            ),
            Action::Move { from, to, lnk } => (
                "move",
                display_name(to),
                run(dry_run, || {
                    write_lnk(to, lnk).map_err(|e| e.to_string())?;
                    if from != to {
                        std::fs::remove_file(from).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                }),
            ),
            Action::DeleteOrphan(path) => (
                "delete",
                display_name(path),
                run(dry_run, || {
                    std::fs::remove_file(path).map_err(|e| e.to_string())
                }),
            ),
            Action::Skip { path, .. } => ("skip", display_name(path), Ok(())),
        };

        outcomes.push(Outcome { action: label.to_string(), name, result });
    }

    outcomes
}

fn run<F>(dry_run: bool, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    if dry_run {
        Ok(())
    } else {
        f()
    }
}

pub fn summarize(outcomes: &[Outcome]) -> Summary {
    let mut s = Summary::default();
    for o in outcomes {
        if o.result.is_err() {
            s.failed += 1;
            continue;
        }
        match o.action.as_str() {
            "create" => s.created += 1,
            "retarget" | "fix-icon" => s.updated += 1,
            "move" => s.moved += 1,
            "delete" => s.deleted += 1,
            _ => s.skipped += 1,
        }
    }
    s
}

/// Directories the tool may own for a given base and organization. Thin
/// re-export so callers do not need to reach into `layout` directly.
pub fn managed_dirs(base: &Path, org: &Organization) -> Vec<PathBuf> {
    layout::managed_dirs(base, org)
}
```

Add `pub mod ops;` to `src/lib.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test ops_apply`
Expected: PASS, 3 tests

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: PASS, 74 tests

- [ ] **Step 6: Commit**

```bash
git add src/ tests/
git commit -m "feat: scan owned shortcuts and apply reconciliation actions"
```

---

### Task 13: CLI wiring

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs`, `src/lib.rs`

**Interfaces:**
- Consumes: everything built so far
- Produces:
  - `cli::Cli` with `command: Option<Command>` and global flags
  - `cli::Command` enum: `Sync`, `Migrate`, `Clean`, `Status`
  - `cli::run(cli: Cli) -> i32` — returns the process exit code

- [ ] **Step 1: Write the failing test**

Create `src/cli.rs`:

```rust
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
        assert_eq!(Cli::parse_from(["wso", "sync"]).effective_command(), Command::Sync);
        assert_eq!(Cli::parse_from(["wso", "clean"]).effective_command(), Command::Clean);
        assert_eq!(Cli::parse_from(["wso", "status"]).effective_command(), Command::Status);
        assert_eq!(
            Cli::parse_from(["wso", "migrate"]).effective_command(),
            Command::Migrate
        );
    }

    #[test]
    fn parses_organization_flags() {
        let cli = Cli::parse_from([
            "wso", "sync", "--organize", "per-platform", "--folder", "Juegos", "--yes",
        ]);
        assert_eq!(cli.organize, Some(OrganizeArg::PerPlatform));
        assert_eq!(cli.folder.as_deref(), Some("Juegos"));
        assert!(cli.yes);
        assert!(!cli.dry_run);
    }

    #[test]
    fn flags_override_config() {
        let cli = Cli::parse_from([
            "wso", "--target", "desktop", "--organize", "unified", "--folder", "Games",
        ]);
        let mut cfg = crate::config::Config::default();
        cli.apply_overrides(&mut cfg);
        assert_eq!(cfg.targets, vec![crate::layout::Target::Desktop]);
        assert_eq!(
            cfg.organization,
            crate::layout::Organization::Unified { folder: "Games".into() }
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
```

```bash
cargo add clap --features derive
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test cli`
Expected: FAIL — `cannot find type 'Cli' in this scope`

- [ ] **Step 3: Write the CLI definition**

Prepend to `src/cli.rs`:

```rust
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
    version,
    about = "Organize Steam and Epic game shortcuts on Windows",
    long_about = "Creates real .lnk shortcuts with correct icons for your installed \
Steam and Epic games, on the Desktop and in the Start Menu.\n\n\
Note: Windows does not allow programs to pin items to the Start menu panel. \
Shortcuts are placed in the Start Menu's Programs folder, where they appear \
under All apps and are searchable; pinning remains a manual right-click."
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

    /// Show what would happen without touching the disk
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
                OrganizeArg::PerPlatform => {
                    Organization::PerPlatform { parent: self.folder.clone() }
                }
            };
        } else if let Some(folder) = &self.folder {
            // A folder name on its own renames the existing unified folder.
            if let Organization::Unified { .. } = cfg.organization {
                cfg.organization = Organization::Unified { folder: folder.clone() };
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test cli`
Expected: PASS, 6 tests

- [ ] **Step 5: Wire up `main.rs`**

Replace `src/main.rs`:

```rust
use clap::Parser;
use winshortcutsorganizer::cli::{Cli, Command};
use winshortcutsorganizer::{commands, shortcut::writer::ComGuard};

fn main() {
    let cli = Cli::parse();

    let _com = match ComGuard::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };

    let code = match cli.effective_command() {
        Command::Sync => commands::sync(&cli),
        Command::Migrate => commands::migrate(&cli),
        Command::Clean => commands::clean(&cli),
        Command::Status => commands::status(&cli),
    };

    std::process::exit(code);
}
```

Create `src/commands.rs` with the four entry points. `sync`, `clean` and
`status` are implemented here; `migrate` returns a placeholder that Task 15
replaces.

```rust
use crate::cli::Cli;
use crate::config;
use crate::layout::{self, Target};
use crate::ops;
use crate::plan::{plan, Action};
use crate::platform::{epic::EpicProvider, steam::SteamProvider, Game, GameProvider, Platform};

fn collect_games(cfg: &config::Config) -> (Vec<Game>, Vec<String>) {
    let mut games = Vec::new();
    let mut notes = Vec::new();

    if cfg.platforms.contains(&Platform::Steam) {
        match SteamProvider::detect() {
            None => notes.push("Steam is not installed; skipping.".to_string()),
            Some(p) => match p.games() {
                Ok(mut g) => games.append(&mut g),
                Err(e) => notes.push(format!("Steam could not be read: {e}")),
            },
        }
    }
    if cfg.platforms.contains(&Platform::Epic) {
        match EpicProvider::detect() {
            None => notes.push("Epic Games Launcher is not installed; skipping.".to_string()),
            Some(p) => match p.games() {
                Ok(mut g) => games.append(&mut g),
                Err(e) => notes.push(format!("Epic could not be read: {e}")),
            },
        }
    }
    (games, notes)
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

/// Runs the reconcile-and-apply cycle for one target directory.
fn reconcile_target(
    cfg: &config::Config,
    games: &[Game],
    target: Target,
    dry_run: bool,
    assume_yes: bool,
    delete_all: bool,
) -> Result<ops::Summary, String> {
    let base = layout::target_dir(target).map_err(|e| e.to_string())?;
    let cache = config::cache_dir();

    let owned = ops::scan_owned(&ops::managed_dirs(&base, &cfg.organization));
    let desired = if delete_all {
        Vec::new()
    } else {
        ops::build_desired(games, cfg, &base, &cache)
    };

    let actions = plan(&desired, &owned);
    let pending: Vec<&Action> = actions
        .iter()
        .filter(|a| !matches!(a, Action::Skip { .. }))
        .collect();

    println!("\n{} — {} change(s) pending", target.label(), pending.len());
    describe(&actions);

    if pending.is_empty() {
        return Ok(ops::summarize(&ops::apply(&actions, true)));
    }
    if !dry_run && !confirm("Apply these changes?", assume_yes) {
        println!("Cancelled.");
        return Ok(ops::Summary::default());
    }

    let outcomes = ops::apply(&actions, dry_run);
    for o in outcomes.iter().filter(|o| o.result.is_err()) {
        eprintln!(
            "  failed to {} {}: {}",
            o.action,
            o.name,
            o.result.as_ref().unwrap_err()
        );
    }
    Ok(ops::summarize(&outcomes))
}

fn run_over_targets(cli: &Cli, delete_all: bool) -> i32 {
    let path = config::config_path();
    let mut cfg = config::load(&path);
    cli.apply_overrides(&mut cfg);

    let (games, notes) = if delete_all {
        (Vec::new(), Vec::new())
    } else {
        collect_games(&cfg)
    };
    for n in &notes {
        println!("{n}");
    }

    let mut failed = 0;
    for target in cfg.targets.clone() {
        match reconcile_target(&cfg, &games, target, cli.dry_run, cli.yes, delete_all) {
            Ok(s) => {
                println!(
                    "  {} created, {} updated, {} moved, {} deleted, {} unchanged, {} failed",
                    s.created, s.updated, s.moved, s.deleted, s.skipped, s.failed
                );
                failed += s.failed;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        }
    }

    if !cli.dry_run && !delete_all {
        if let Err(e) = config::save(&path, &cfg) {
            eprintln!("Warning: could not save preferences: {e}");
        }
    }
    exit_code(failed)
}

pub fn sync(cli: &Cli) -> i32 {
    if !cli.is_non_interactive() {
        let path = config::config_path();
        let mut cfg = config::load(&path);
        if crate::wizard::run(&mut cfg).is_err() {
            println!("Cancelled.");
            return 0;
        }
        if let Err(e) = config::save(&path, &cfg) {
            eprintln!("Warning: could not save preferences: {e}");
        }
    }
    run_over_targets(cli, false)
}

pub fn clean(cli: &Cli) -> i32 {
    println!("This removes every shortcut WinShortcutsOrganizer created.");
    println!("Shortcuts you made yourself are never touched.");
    if !cli.dry_run && !confirm("Continue?", cli.yes) {
        println!("Cancelled.");
        return 0;
    }
    run_over_targets(cli, true)
}

pub fn status(cli: &Cli) -> i32 {
    let mut cfg = config::load(&config::config_path());
    cli.apply_overrides(&mut cfg);

    let (games, notes) = collect_games(&cfg);
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
    0
}

pub fn migrate(_cli: &Cli) -> i32 {
    println!("migrate is implemented in Task 15");
    0
}
```

Add to `src/lib.rs`:

```rust
pub mod cli;
pub mod commands;
pub mod config;
pub mod ops;
pub mod wizard;
```

```bash
cargo add inquire
```

- [ ] **Step 6: Verify it builds and the suite passes**

Run: `cargo build && cargo test`
Expected: build fails until Task 14 provides `wizard::run`. Create
`src/wizard.rs` with a stub now so the tree compiles:

```rust
//! Interactive prompts. Fully implemented in Task 14.

use crate::config::Config;

pub struct Cancelled;

pub fn run(_cfg: &mut Config) -> Result<(), Cancelled> {
    Ok(())
}
```

Then run `cargo build && cargo test` again.
Expected: PASS, 80 tests

- [ ] **Step 7: Commit**

```bash
git add src/ Cargo.toml Cargo.lock
git commit -m "feat: add CLI with sync, clean and status commands"
```

---

### Task 14: Interactive wizard

**Files:**
- Modify: `src/wizard.rs`

**Interfaces:**
- Consumes: `config::Config`, `layout::{Organization, Target}`, `platform::Platform`
- Produces: `wizard::run(cfg: &mut Config) -> Result<(), Cancelled>`

- [ ] **Step 1: Write the failing test for the pure part**

The prompts themselves cannot be unit tested, so the translation from answers
to configuration is extracted into a pure function that can be.

Replace `src/wizard.rs` with its test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Organization, Target};
    use crate::platform::Platform;

    #[test]
    fn builds_config_from_answers() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam],
                targets: vec![Target::Desktop, Target::StartMenu],
                organization: OrgChoice::PerPlatform,
                folder: Some("Juegos".into()),
            },
        );
        assert_eq!(cfg.platforms, vec![Platform::Steam]);
        assert_eq!(cfg.targets.len(), 2);
        assert_eq!(
            cfg.organization,
            Organization::PerPlatform { parent: Some("Juegos".into()) }
        );
    }

    #[test]
    fn flat_mode_ignores_the_folder_name() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam, Platform::Epic],
                targets: vec![Target::StartMenu],
                organization: OrgChoice::Flat,
                folder: Some("Ignored".into()),
            },
        );
        assert_eq!(cfg.organization, Organization::Flat);
    }

    #[test]
    fn unified_mode_defaults_the_folder_name() {
        let mut cfg = Config::default();
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Epic],
                targets: vec![Target::StartMenu],
                organization: OrgChoice::Unified,
                folder: None,
            },
        );
        assert_eq!(
            cfg.organization,
            Organization::Unified { folder: "Games".into() }
        );
    }

    #[test]
    fn preserves_exclusions_and_overrides() {
        let mut cfg = Config::default();
        cfg.exclusions = vec!["steam:1".into()];
        apply_answers(
            &mut cfg,
            Answers {
                platforms: vec![Platform::Steam],
                targets: vec![Target::Desktop],
                organization: OrgChoice::Flat,
                folder: None,
            },
        );
        assert_eq!(cfg.exclusions, vec!["steam:1".to_string()]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test wizard`
Expected: FAIL — `cannot find type 'Answers' in this scope`

- [ ] **Step 3: Write the implementation**

Prepend to `src/wizard.rs`:

```rust
//! Interactive prompts. The translation from answers to configuration is kept
//! pure and separate from the prompting so it can be tested.

use crate::config::Config;
use crate::layout::{Organization, Target};
use crate::platform::Platform;

pub struct Cancelled;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgChoice {
    Flat,
    Unified,
    PerPlatform,
}

#[derive(Debug, Clone)]
pub struct Answers {
    pub platforms: Vec<Platform>,
    pub targets: Vec<Target>,
    pub organization: OrgChoice,
    pub folder: Option<String>,
}

pub fn apply_answers(cfg: &mut Config, a: Answers) {
    cfg.platforms = a.platforms;
    cfg.targets = a.targets;
    cfg.organization = match a.organization {
        OrgChoice::Flat => Organization::Flat,
        OrgChoice::Unified => Organization::Unified {
            folder: a.folder.unwrap_or_else(|| "Games".into()),
        },
        OrgChoice::PerPlatform => Organization::PerPlatform { parent: a.folder },
    };
}

pub fn run(cfg: &mut Config) -> Result<(), Cancelled> {
    use inquire::{Confirm, MultiSelect, Select, Text};

    println!("WinShortcutsOrganizer\n");

    let platform_labels = vec!["Steam", "Epic"];
    let chosen = MultiSelect::new("Which platforms?", platform_labels)
        .with_default(&[0, 1])
        .prompt()
        .map_err(|_| Cancelled)?;
    if chosen.is_empty() {
        println!("No platforms selected.");
        return Err(Cancelled);
    }
    let platforms: Vec<Platform> = chosen
        .iter()
        .filter_map(|l| match *l {
            "Steam" => Some(Platform::Steam),
            "Epic" => Some(Platform::Epic),
            _ => None,
        })
        .collect();

    let target_choice = Select::new(
        "Where should shortcuts go?",
        vec!["Start Menu", "Desktop", "Both"],
    )
    .prompt()
    .map_err(|_| Cancelled)?;
    let targets = match target_choice {
        "Desktop" => vec![Target::Desktop],
        "Both" => vec![Target::Desktop, Target::StartMenu],
        _ => vec![Target::StartMenu],
    };

    let org_choice = Select::new(
        "How should they be grouped?",
        vec![
            "One folder for all games",
            "One folder per platform",
            "No folder, place them directly",
        ],
    )
    .prompt()
    .map_err(|_| Cancelled)?;
    let organization = match org_choice {
        "One folder per platform" => OrgChoice::PerPlatform,
        "No folder, place them directly" => OrgChoice::Flat,
        _ => OrgChoice::Unified,
    };

    let folder = match organization {
        OrgChoice::Flat => None,
        OrgChoice::Unified => Some(
            Text::new("Folder name:")
                .with_default("Games")
                .prompt()
                .map_err(|_| Cancelled)?,
        ),
        OrgChoice::PerPlatform => {
            let nest = Confirm::new("Nest the platform folders inside a parent folder?")
                .with_default(true)
                .prompt()
                .map_err(|_| Cancelled)?;
            if nest {
                Some(
                    Text::new("Parent folder name:")
                        .with_default("Games")
                        .prompt()
                        .map_err(|_| Cancelled)?,
                )
            } else {
                None
            }
        }
    };

    apply_answers(cfg, Answers { platforms, targets, organization, folder });

    if cfg.targets.contains(&Target::StartMenu) {
        println!(
            "\nNote: Windows does not let programs pin items to the Start menu panel.\n\
             Your games will appear under All apps and in search; pinning is a manual\n\
             right-click from there."
        );
    }

    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS, 84 tests

- [ ] **Step 5: Exercise the wizard by hand**

Run: `cargo run -- --dry-run`
Expected: prompts appear, then a list of pending changes, and nothing is
written to disk. Confirm the Start Menu note is printed.

- [ ] **Step 6: Commit**

```bash
git add src/
git commit -m "feat: add interactive setup wizard"
```

---

### Task 15: Migrating legacy .url shortcuts

**Files:**
- Create: `src/shortcut/url_legacy.rs`
- Modify: `src/shortcut/mod.rs`, `src/commands.rs`

**Interfaces:**
- Consumes: `shortcut::writer`, `shortcut::signature`, `platform::steam`
- Produces:
  - `shortcut::url_legacy::LegacyUrl { appid: String, icon_file: Option<PathBuf>, path: PathBuf }`
  - `shortcut::url_legacy::parse(path: &Path, contents: &str) -> Option<LegacyUrl>`
  - `shortcut::url_legacy::find(dirs: &[PathBuf]) -> Vec<LegacyUrl>`

- [ ] **Step 1: Write the failing test**

Create `src/shortcut/url_legacy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const STEAM_URL: &str = "[InternetShortcut]\r\n\
URL=steam://rungameid/105600\r\n\
IconIndex=0\r\n\
IconFile=C:\\Program Files (x86)\\Steam\\steam\\games\\abc123.ico\r\n";

    #[test]
    fn parses_appid_and_icon() {
        let p = PathBuf::from(r"C:\Desktop\Terraria.url");
        let parsed = parse(&p, STEAM_URL).unwrap();
        assert_eq!(parsed.appid, "105600");
        assert_eq!(
            parsed.icon_file,
            Some(PathBuf::from(
                r"C:\Program Files (x86)\Steam\steam\games\abc123.ico"
            ))
        );
    }

    #[test]
    fn parses_without_an_icon_line() {
        let text = "[InternetShortcut]\r\nURL=steam://rungameid/440\r\n";
        let parsed = parse(Path::new("x.url"), text).unwrap();
        assert_eq!(parsed.appid, "440");
        assert_eq!(parsed.icon_file, None);
    }

    #[test]
    fn accepts_the_applaunch_form() {
        let text = "[InternetShortcut]\r\nURL=steam://launch/620\r\n";
        assert_eq!(parse(Path::new("x.url"), text).unwrap().appid, "620");
    }

    #[test]
    fn ignores_non_steam_urls() {
        let text = "[InternetShortcut]\r\nURL=https://example.com\r\n";
        assert!(parse(Path::new("x.url"), text).is_none());
    }

    #[test]
    fn ignores_a_rungameid_with_a_mod_suffix() {
        // Non-Steam shortcuts use 64-bit composite ids; those are not appids.
        let text = "[InternetShortcut]\r\nURL=steam://rungameid/14829735431807303680\r\n";
        assert!(parse(Path::new("x.url"), text).is_none());
    }
}
```

Add `pub mod url_legacy;` to `src/shortcut/mod.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test url_legacy`
Expected: FAIL — `cannot find function 'parse' in this scope`

- [ ] **Step 3: Write the implementation**

Prepend to `src/shortcut/url_legacy.rs`:

```rust
use std::path::{Path, PathBuf};

/// A shortcut Steam created, which is a web bookmark rather than a real
/// shortcut. These are what lose their icons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyUrl {
    pub path: PathBuf,
    pub appid: String,
    /// Steam already resolved an icon for this shortcut. When the file still
    /// exists it is a better source than any heuristic, so migration reuses it.
    pub icon_file: Option<PathBuf>,
}

/// Steam appids fit comfortably in 32 bits. `rungameid` values longer than
/// this are composite ids for non-Steam shortcuts and mods, which cannot be
/// launched with `-applaunch`.
const MAX_APPID_DIGITS: usize = 10;

pub fn parse(path: &Path, contents: &str) -> Option<LegacyUrl> {
    let mut appid = None;
    let mut icon_file = None;

    for line in contents.lines() {
        let line = line.trim();
        if let Some(url) = line.strip_prefix("URL=") {
            let tail = url
                .strip_prefix("steam://rungameid/")
                .or_else(|| url.strip_prefix("steam://launch/"))?;
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || digits.len() > MAX_APPID_DIGITS {
                return None;
            }
            appid = Some(digits);
        } else if let Some(icon) = line.strip_prefix("IconFile=") {
            if !icon.is_empty() {
                icon_file = Some(PathBuf::from(icon));
            }
        }
    }

    Some(LegacyUrl { path: path.to_path_buf(), appid: appid?, icon_file })
}

pub fn find(dirs: &[PathBuf]) -> Vec<LegacyUrl> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("url") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(parsed) = parse(&path, &text) {
                out.push(parsed);
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test url_legacy`
Expected: PASS, 5 tests

- [ ] **Step 5: Implement the migrate command**

Replace the placeholder `migrate` in `src/commands.rs`:

```rust
pub fn migrate(cli: &Cli) -> i32 {
    use crate::icon;
    use crate::shortcut::signature::{encode, Signature};
    use crate::shortcut::url_legacy;
    use crate::shortcut::{writer::write_lnk, LnkData};

    let Some(steam) = SteamProvider::detect() else {
        eprintln!("Steam is not installed; nothing to migrate.");
        return 2;
    };

    // Legacy .url files land wherever Steam put them, which is the target
    // roots themselves rather than any folder we manage.
    let mut roots = Vec::new();
    for target in [Target::Desktop, Target::StartMenu] {
        if let Ok(dir) = layout::target_dir(target) {
            roots.push(dir);
        }
    }

    let found = url_legacy::find(&roots);
    if found.is_empty() {
        println!("No Steam .url shortcuts found on the Desktop or in the Start Menu.");
        return 0;
    }

    println!("Found {} Steam .url shortcut(s):", found.len());
    for f in &found {
        println!("  {}", f.path.display());
    }
    println!("\nEach will be replaced by a .lnk with a working icon.");

    if !cli.dry_run && !confirm("Convert them now?", cli.yes) {
        println!("Cancelled.");
        return 0;
    }

    let games = steam.games().unwrap_or_default();
    let cache = config::cache_dir();
    let mut failed = 0;

    for legacy in &found {
        let name = legacy
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Game")
            .to_string();

        let sig = Signature { platform: Platform::Steam, id: legacy.appid.clone() };
        let game = games.iter().find(|g| g.id == legacy.appid);

        // Steam already resolved this shortcut's icon; prefer it over the
        // heuristic, then fall back to the normal candidate chain.
        let resolved = legacy
            .icon_file
            .as_ref()
            .filter(|p| p.is_file())
            .map(|p| icon::ResolvedIcon { path: p.clone(), index: 0 })
            .or_else(|| {
                game.and_then(|g| {
                    icon::resolve(
                        &g.icon_candidates,
                        &cache,
                        &format!("steam_{}", legacy.appid),
                    )
                })
            });

        let data = LnkData {
            target: steam.root().join("steam.exe"),
            args: format!("-applaunch {}", legacy.appid),
            working_dir: Some(steam.root().to_path_buf()),
            icon: resolved.map(|r| (r.path, r.index)),
            description: encode(game.map(|g| g.name.as_str()).unwrap_or(&name), &sig),
        };

        let dest = legacy.path.with_extension("lnk");

        if cli.dry_run {
            println!("  would convert {}", legacy.path.display());
            continue;
        }
        match write_lnk(&dest, &data) {
            Ok(()) => {
                if let Err(e) = std::fs::remove_file(&legacy.path) {
                    eprintln!("  converted {name}, but could not remove the .url: {e}");
                    failed += 1;
                } else {
                    println!("  converted {name}");
                }
            }
            Err(e) => {
                eprintln!("  failed to convert {name}: {e}");
                failed += 1;
            }
        }
    }

    exit_code(failed)
}
```

- [ ] **Step 6: Verify against the real machine**

Run: `cargo run -- migrate --dry-run`
Expected: either a list of `.url` shortcuts that would be converted, or
"No Steam .url shortcuts found". Nothing is written either way.

- [ ] **Step 7: Commit**

```bash
git add src/
git commit -m "feat: migrate legacy Steam .url shortcuts to .lnk"
```

---

### Task 16: Documentation and release

**Files:**
- Create: `README.md`, `LICENSE`, `.github/workflows/release.yml`

- [ ] **Step 1: Write the README**

Create `README.md`. It must state the pinning limitation prominently, since
that is the expectation most likely to be violated:

```markdown
# WinShortcutsOrganizer

Creates real Windows shortcuts, with correct icons, for the games you already
have installed through Steam and the Epic Games Launcher.

## Why

Steam creates `.url` files, which are web bookmarks rather than shortcuts.
They cannot hold a stable icon, so they show up as blank white squares in the
Start Menu. Epic does not create Start Menu shortcuts at all.

`wso` creates `.lnk` shortcuts instead. A `.lnk` keeps its icon separate from
what it launches, so a shortcut can start your game through Steam while
displaying the game's own icon.

## What it does not do

**It cannot pin anything to the Start menu panel.** Windows removed
programmatic pinning in Windows 10 1607, and no tool can do it. `wso` places
shortcuts in the Start Menu's Programs folder, where they appear under
*All apps* and are searchable by typing. Pinning to the panel is still a
manual right-click from there.

## Install

Download `wso.exe` from the [releases page](../../releases) and run it. No
installation, no administrator rights, no runtime to install.

## Use

Run `wso.exe` with no arguments for an interactive setup.

| Command | What it does |
|---|---|
| `wso sync` | Create and update shortcuts. Safe to re-run. |
| `wso migrate` | Convert Steam's broken `.url` shortcuts to `.lnk`. |
| `wso clean` | Remove every shortcut `wso` created. |
| `wso status` | Show what exists and what is out of sync. |

Add `--dry-run` to any command to see what would happen without changing
anything.

### Flags

    --target      desktop | start-menu | both
    --organize    flat | unified | per-platform
    --folder      folder name for unified or per-platform mode
    --platforms   steam,epic
    --dry-run     show changes without applying them
    --yes         skip confirmation prompts

## Safety

`wso` marks every shortcut it creates. `clean` deletes only marked shortcuts,
and `sync` never touches a file it did not create. Shortcuts you made by hand
are always left alone.

## Configuration

Preferences live in `%APPDATA%\WinShortcutsOrganizer\config.toml`. You can
exclude games and override icons there:

```toml
exclusions = ["steam:228980", "Wallpaper Engine"]

[icon_overrides]
"steam:440" = "C:\\icons\\tf2.ico"
```

## Building from source

    cargo build --release

Requires Rust and Windows.

## License

MIT
```

- [ ] **Step 2: Add the license**

Create `LICENSE` with the standard MIT license text, `Copyright (c) 2026`.

- [ ] **Step 3: Add a release workflow**

Create `.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    tags: ['v*']

jobs:
  build:
    runs-on: windows-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --all
      - run: cargo build --release
      - uses: softprops/action-gh-release@v2
        with:
          files: target/release/wso.exe
          body: |
            Download `wso.exe` and run it. No installation required.

            Windows SmartScreen may warn about an unsigned executable on first
            run; choose "More info" then "Run anyway".
```

- [ ] **Step 4: Verify the release build**

Run: `cargo build --release`
Expected: `target/release/wso.exe` exists and is under 5 MB.

Run: `cargo test --all`
Expected: PASS, 89 tests

- [ ] **Step 5: End-to-end verification on the real machine**

Run each of these and confirm the output before moving on:

```bash
cargo run --release -- status
```

Expected: reports the installed Steam games, and that Epic is either detected
or reported as not installed.

```bash
cargo run --release -- sync --target desktop --organize unified --folder "WSO Test" --dry-run
```

Expected: a list of `create` actions under a `WSO Test` folder, and nothing
written to the Desktop.

Then run it for real, confirm the icons are correct in Explorer, confirm at
least one shortcut launches its game, and finally:

```bash
cargo run --release -- clean --yes
```

Expected: the `WSO Test` folder's shortcuts are gone and no other Desktop
shortcut was touched.

- [ ] **Step 6: Commit**

```bash
git add README.md LICENSE .github/
git commit -m "docs: add README, license and release workflow"
```

---

## Verification

Before declaring the project complete, confirm each of these by running the
command and reading the output — not by assuming:

- [ ] `cargo test --all` passes, including the eight executable-finder fixtures
- [ ] `cargo build --release` produces `wso.exe`
- [ ] `wso status` lists real games from the real Steam installation
- [ ] `wso sync --dry-run` writes nothing to disk
- [ ] A shortcut created by `wso sync` shows the game's icon and launches it
- [ ] A hand-made shortcut placed in a managed folder survives `wso clean`
- [ ] `wso` was never run with administrator rights during any of the above
