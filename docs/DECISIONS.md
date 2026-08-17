# Decisions taken on the maintainer's behalf

This project was executed by an autonomous agent loop. Where the plan was silent,
wrong, or in conflict with the spec, the controller decided rather than stopping,
and recorded every such decision together with what it costs if the decision was
wrong.

This document exists so those decisions can be found and reversed. It is
exhaustive: every ruling made during execution appears below, in the order it was
made. Nothing was decided silently.

The design document is the authority on intent:
`docs/superpowers/specs/2026-08-16-winshortcuts-organizer-design.md`.

---

### 1

`clean` scans only the directories implied by the CURRENT config, so shortcuts left behind under a previously-configured organization mode would be missed — a real gap in the plan's Task 13, since `clean` is advertised as "remove every shortcut this tool created". Decision: `clean` scans the union of every layout the tool can produce (flat, unified, and per-platform, under both Desktop and Start Menu bases), not just the configured one; ownership signatures still gate every deletion, so widening the scan cannot endanger foreign files. Carried into the Task 13 dispatch.

**Cost if wrong:** `clean` reads a few extra directories per run, costing milliseconds; no deletion risk, because the signature check is unchanged.

### 2

the per-task "Expected: PASS, N tests" counts in the plan are my own arithmetic and may drift by a test or two as implementers add coverage. Decision: treat a count mismatch as informational, not a failure, provided the suite is green and the task's named tests exist.

**Cost if wrong:** a genuinely missing test could hide behind a drifting count, which the task review's spec check is expected to catch independently.

### 3

the smoke build emits dead-code warnings (`sanitize_filename` and `RESERVED` never used), because the plan makes the crate bin-only until Task 4 introduces `src/lib.rs`, so nothing consumes `layout` yet. The implementer contract demands pristine test output, so this is a plan-versus-rubric conflict. Decision: pull `src/lib.rs` forward from Task 4 into Task 1 — `pub mod layout;` makes the function a public library item, which is not dead code, removing the warning at its root rather than suppressing it with an allow attribute. `src/main.rs` drops its `mod layout;` declaration so the module is not compiled into both targets. Task 4's step that creates `src/lib.rs` becomes an extension of the existing file instead of a creation; every later task that adds a `pub mod` line to lib.rs is unaffected.

**Cost if wrong:** Task 4's brief will describe creating a file that already exists, which its implementer must reconcile; the ledger entry is the warning that this is expected.

### 4

the finding stands and the plan text is wrong. The spec's binding guarantee is that clean deletes only marked shortcuts and sync never touches a file it did not create, and decode is the sole enforcement point for it; a decode that guesses is a safety defect, not a style preference. Fixing rather than parking, even though a real collision needs a foreign description that both contains "[wso:" and ends with "]".

**Cost if wrong:** one extra rejection clause and two tests, which is trivial; the opposite error deletes a user's own file.

### 5

the quadratic string scanner is real and I accept it. The Some(_) arm calls from_utf8 over the ENTIRE remaining buffer to decode one character, so cost is O(remaining) per character and O(n^2) per file. I regard the reviewer's Critical severity as high for this project's realistic inputs — Steam's text VDF files are kilobytes, where the quadratic term is invisible — but severity does not change the action: the algorithm is simply wrong and the fix (decode width from the lead byte, slice a bounded window) is a few lines. Fixing, not debating.

**Cost if wrong:** a few lines of lead-byte width logic that a reviewer must check for off-by-one.

### 6

unbounded recursion accepted as Important. parse_entries recurses per nested '{' with no cap; a corrupted file with thousands of braces aborts the process via stack overflow, which in Rust is not a catchable panic. Real Steam files nest three or four levels. Adding a depth cap with a dedicated error.

**Cost if wrong:** a legitimate file deeper than the cap is rejected; the cap is set an order of magnitude above anything Steam produces.

### 7

escape path UTF-8 bug and the missing non-ASCII test both accepted. The global constraint names Overwatch(R) and HELLDIVERS(TM) 2 explicitly, yet not one of the six brief-supplied tests parses a non-ASCII byte — my plan specified tests that never exercise the constraint it declared. That gap is why the escape bug survived.

**Cost if wrong:** none; the tests assert real required behavior.

### 8

both Important findings accepted. The size tiebreaker and the entire DIR_BLOCKLIST branch have zero test witnesses anywhere in 32 tests — flipping size to Reverse(size), or deleting it from the tuple, would not fail a single test. For the algorithm that picks the icon of every Steam shortcut, an untested branch is not "coverage could be broader"; it is a rule nobody has ever demonstrated works. No production code changes needed, only two targeted tests.

**Cost if wrong:** two synthetic tests in a suite otherwise built from real-library fixtures, which slightly dilutes the "these are all real cases" property of the file.

### 9

do not reopen Task 4's fix loop for this. The algorithm did exactly what it was specified to do, and the right layer to handle it is the icon chain in Task 7: if busybox64.exe carries no icon resource, has_icon() rejects it and the chain falls through to the librarycache artwork, which is the correct outcome and needs no code change. Carrying tModLoader forward as a MANDATORY real-machine verification case for Task 7 — if busybox64.exe does have an icon, the fix is a blocklist entry added then, with the evidence in hand.

**Cost if wrong:** one Steam game out of nine shows a BusyBox icon until Task 7, and the user can already override it via config icon_overrides.

### 10

the Important is DISMISSED, and the fault was mine. I told the reviewer "any move toward reading Steam's icon cache is scope creep", which overstates the spec. Spec line 213 explicitly mandates librarycache as the second fallback; what is forbidden is appinfo.vdf, the binary format. The code is correct as written and no change follows from this finding. The reviewer was right to raise it given what I had told it.

**Cost if wrong:** none to the code; the lesson is that a constraint block copied loosely instead of verbatim turns into a false finding the next reviewer has to spend effort on.

### 11

the reviewer's "cannot verify" item turned out to be the real defect, and I accept it. It asked whether librarycache/<appid>/logo.png is actually the filename Steam uses. Checked all nine real appids: six have logo.png, three (553850 Helldivers 2, 2246340 Monster Hunter Wilds, 2357570 Overwatch) do not — their directories hold content-hash filenames, several with no extension, which is how current Steam clients store library assets. So the hardcoded filename is wrong for a third of a real library. Fixing: build_game must scan the appid directory and pick the best image rather than assume a name. Spec corrected and committed separately.

**Cost if wrong:** a directory scan per game at build time, which is one readdir on a directory of six or so files.

### 12

Important findings 1-3 accepted. All three are the same failure this project keeps hitting — a condition or value that no test would notice the absence of. Specifically: the non-empty-launch-executable gate can be deleted with all 52 tests still green; the quoted wrapper that actually reaches the .lnk is only substring-checked, so quoting could be dropped or doubled undetected; and there is no Epic equivalent of steam.rs's rejects_a_manifest_missing_fields, so the skip-malformed-and-continue guard is inferred from reading rather than executed. On the one platform nobody can run against a real installation, inference is not good enough.

**Cost if wrong:** three added tests and one spec line.

### 13

the non-empty-launch-executable condition is KEPT, not dropped, though the reviewer correctly noted it is absent from the spec. An entry with no launch executable yields no icon source and is almost certainly not a playable title. Recording it in the spec rather than deleting it from the code.

**Cost if wrong:** a hypothetical Epic game that declares no executable would be hidden from the tool.

### 14

add BOTH "busybox" and "quickeditdisable" to NAME_BLOCKLIST, and add tModLoader to the regression corpus as the first entry whose expected pick is None. Blocklisting busybox alone would also produce the right icon today, because QuickEditDisable has no icon and the probe would reject it — but that outcome would be accidental rather than designed, and a future build of that helper gaining an icon would silently break it. Having the finder answer None is the truthful answer for a game that genuinely has no representative executable, and it makes the corpus cover the none-of-the-above case with real data. Both names are generic utilities that never identify a game, which is exactly what NAME_BLOCKLIST is for. Folding into Task 7's fix round rather than a separate round.

**Cost if wrong:** a hypothetical game whose real executable contains "busybox" or "quickeditdisable" in its filename would be excluded.

### 15

the partial-write cache poisoning is real and I accept it. File::create truncates before ico::write serializes, so an I/O failure part-way through leaves an empty or half-written .ico at the destination; resolve()'s cache check only tests is_file(), so that corrupt file is served forever with no retry and no signal. The common failure (an undecodable source) is safe because decoding happens before any file is created, but disk-full, permissions and antivirus locks are ordinary, not exotic. Fix is write-to-temp-then-rename.

**Cost if wrong:** one extra file create and a rename per generated icon.

### 16

the unsanitized cache key is real and I accept it, even though it is not reachable today. The reviewer traced the provenance correctly: key derives from Game.id, which is a VDF-parsed appid on Steam and a JSON-parsed AppName on Epic, neither validated against a charset. cache_dir.join(format!("{key}.ico")) with a key containing separators or ".." writes outside the cache directory. No caller wires it yet, which is why it is Important rather than Critical — but Task 12 wires exactly that, and fixing a contract before it has callers is far cheaper than after. Decision: validate and REJECT rather than sanitize-and-continue. Rejecting skips the image candidate and falls through to the platform fallback icon, which is safe degradation; sanitizing by character replacement could map two distinct ids onto one cache file and show the wrong game's icon, which is a worse failure than a generic one.

**Cost if wrong:** a game whose id contains unusual characters gets the launcher's icon instead of its artwork.

### 17

add "busybox" and "quickeditdisable" to NAME_BLOCKLIST and add tModLoader to the regression corpus as the first entry expecting None. Probed the real machine: busybox64.exe has an icon, so the chain accepts it and tModLoader would ship showing BusyBox's icon; QuickEditDisable.exe has none. Blocklisting busybox alone would produce the right result today only because the probe rejects QuickEditDisable — accidental, not designed, and a future build of that helper gaining an icon would silently break it. Folding into this task's fix round.

### 18

the silent-truncation finding is accepted, but reframed. read_lnk uses 4096-unit buffers and the Win32 Get* APIs truncate silently rather than reporting the needed size, so there is no way to DETECT truncation after the fact. Two directions matter and they are not symmetric. A truncated FOREIGN description loses its trailing marker, decode returns None, and we treat the shortcut as not ours — which is the safe direction and the correct outcome anyway. A truncated shortcut of OUR OWN would lose ownership, become an orphan we never clean, and get duplicated on the next sync. That second case is the real risk, and it is entirely under our control at WRITE time. Decision: enforce the invariant where it can be enforced — write_lnk validates each field against the capacity read_lnk uses and returns an error rather than writing a shortcut it cannot read back. Document on read_lnk that over-long foreign values truncate and why that fails safe. Require an empirical near-limit test FIRST, because the shell may cap description at INFOTIPSIZE (1024) well below the 4096 buffer, in which case the validation threshold must match the observed limit rather than the buffer size.

**Cost if wrong:** a legitimate but absurdly long field is refused with a clear error instead of being silently mangled.

### 19

the measurement exposes a design consequence nobody had considered. Our description is "{title} — managed by WinShortcutsOrganizer [wso:{platform}:{id}]". Fixed overhead is about 55 units for Steam and up to 79 for Epic, whose AppName can be a 32-character hash, leaving roughly 180-205 units for the title. A game whose title exceeds that now makes write_lnk REJECT the shortcut outright, so a legitimate game would silently get no shortcut at all — a worse outcome than the truncation we were protecting against. Decision: signature::encode must budget the description, truncating the human-readable TITLE so the total fits in 260 while the [wso:...] marker always survives intact. The title half is decoration; the marker is the ownership guarantee. Truncation must count UTF-16 units, not chars, and must not split a surrogate pair — game titles contain emoji.

**Cost if wrong:** a very long title shows abbreviated in the tooltip.

### 20

the CoTaskMemFree leak is accepted. SHGetKnownFolderPath allocates a buffer the caller must free, and the `?` on the to_string() conversion returns before the free runs, so an invalid-UTF-16 known folder path leaks. Rare, and a leak rather than corruption, but it is one of the six risks I explicitly asked to be verified and the implementer's report asserted "no leak" — a wrong claim about the exact thing under scrutiny.

**Cost if wrong:** none; freeing unconditionally is strictly safer.

### 21

I am folding in one MINOR against my usual discipline, and stating why. managed_dirs hardcodes Steam and Epic with no compiler-enforced exhaustiveness, so adding a third platform would silently leave its directory out of the scan and clean could never find those shortcuts. The spec's design goal is that adding a provider is a new file rather than a refactor; a silent gap in clean would break that property, so this is a latent correctness gap in a stated architectural guarantee rather than polish. Three lines to fix.

**Cost if wrong:** a small amount of scope added to a fix round that was otherwise single-item.

### 22

duplicate signatures on the DESIRED side are accepted as a real defect, not a documentation gap. The reviewer's trace is right — each desired entry runs its own position() lookup, which does not consume the match, so two desired entries sharing a signature both resolve to the same owned index and both emit an action against it, producing two Move actions with the same `from`. The implementer judged this another layer's responsibility; I disagree, and the reason is concrete rather than theoretical. Steam can leave a stale appmanifest_<appid>.acf in an old library after a game is moved, and Task 5's games() iterates every library without deduplicating by appid, so the same appid can legitimately appear twice. The is_dir filter catches most stale manifests but not one whose folder still exists partially. plan() is the pure core and must not silently act twice on one file. Decision: first desired entry with a given signature wins; subsequent duplicates emit Skip with a named reason so status can report them. Carry-forward to Task 12: build_desired should also deduplicate at source, so the condition is handled where it originates as well as where it would do damage.

**Cost if wrong:** a genuinely duplicated game is reported as skipped rather than acted on twice, which is the safer failure.

### 23

the description-staleness reasoning is accepted as wrong and the fix is to compare descriptions rather than document a caveat. The implementer argued a retitled game always changes its filename and so forces a rewrite; the reviewer showed sanitize_filename is many-to-one, so a title change confined to punctuation or whitespace that sanitization strips yields the same filename, the same path, a Skip, and a description stale forever. Impact is only the tooltip, since ownership depends on the marker and not the title — but the certainty in the report was unjustified, and adding description to the compared fields costs nothing and cannot churn, because both sides are produced by the same deterministic encode.

**Cost if wrong:** a retitled game rewrites its shortcut once.

### 24

silent degradation is right for a MISSING config and wrong for a CORRUPT one. The concrete loss: a user hand-edits the file, makes a typo, runs sync, silently gets default preferences — possibly a different organization mode, so shortcuts land somewhere else — and then save() overwrites their exclusions and icon overrides permanently. Decision: load must distinguish the two cases and report which happened, and save must preserve an unparseable file by renaming it aside before writing, so the preservation is structural rather than dependent on a caller remembering to check. The implementer reached the same conclusion independently and flagged it rather than staying silent.

### 25

the non-atomic write is accepted and folded in with my pending corrupt-config decision, because together they are worse than separately. Under my rename-aside rule alone, a crash during the subsequent write would leave the user with a renamed .invalid file AND a truncated config.toml — worse than before the fix. Decision: save writes to a sibling temp file and renames into place, so the destination is never partially written.

**Cost if wrong:** one extra file create and rename per save.

### 26

the rescue must never overwrite a previous rescue, and when it cannot find a free name it must REFUSE TO SAVE rather than destroy anything. Refusing is inconvenient — the user must clean up before the tool works again — but the alternative is silently discarding the recovery file they were relying on. A tool that declines to act is recoverable; one that quietly deletes the backup is not.

**Cost if wrong:** a user with many accumulated rescue files gets an error telling them to clean up.

### 27

the test-honesty gap on write_atomically's cleanup branch is folded in despite being Minor. The test named "no stray temp file on a failing save" fails before write_atomically is ever called, so deleting its cleanup lines would not fail any test. That is the exact pathology this project has hit eleven times, and closing it costs one test.

**Cost if wrong:** a slightly larger fix diff.

### 28

outcome must not depend on scan order. The implementer's path sort makes it deterministic and happens to pick correctly here only because "Games" sorts before "old" — accidental, not a guarantee. Decision: when several owned entries share a signature, plan() prefers the one whose path equals the desired path, so recovery is correct regardless of ordering. This touches plan.rs, which belongs to a closed task; I am authorising that explicitly rather than reopening Task 10, because the defect was only observable once the applying layer existed.

**Cost if wrong:** a preference rule in the matcher that never fires in the normal single-entry case.

### 29

the case-sensitive .lnk extension match is also folded in. Windows filesystems are case-insensitive, so a shortcut named Game.LNK would be invisible to scan_owned — never claimed, so clean could not remove it and sync would create a duplicate beside it. The codebase already uses eq_ignore_ascii_case for the .exe match in exe_finder, so this is an internal inconsistency as well as a gap.

### 30

the reviewer found dry_run is less inert than my brief claimed. build_desired calls icon::resolve, which writes converted .ico files to the cache, and build_desired runs before plan/apply and takes no dry_run flag. So a dry run does not touch the user's shortcuts but does create the tool's own cache directory and contents. Decision: correct the CLAIM rather than the behaviour. The promise that matters is "I will not modify your shortcuts", and that holds; writing to the tool's own internal cache is no more objectionable than a compiler populating its cache under a dry run. Threading the flag through build_desired would complicate an interface for no user-visible benefit. Carrying forward to Task 13: the --dry-run help text and the README must say what it actually does.

**Cost if wrong:** a dry run leaves an icon cache the user did not expect, costing disk space measured in kilobytes.

### 31

my preliminary view on the exit-2 platform question is confirmed and sharpened. The reviewer traced that collect_games returns Err the moment either provider fails, and since Steam is collected first, a broken Epic install discards the healthy Steam results already gathered. So the user gets a global failure and zero Steam shortcuts because their Epic install is damaged. That is worse than aborting — it is aborting after doing work and throwing it away. Decision: a platform that is detected but unreadable is reported as a failed item; the run continues with the other platform and exits 1. Exit 2 is reserved for cases where the tool provably cannot act at all, such as COM initialisation failing.

### 32

the silent-skip defect is real in BOTH clean and status, not just clean. Each iterates targets with a let-else that drops an unresolvable base and continues, then reports a normal-looking summary. clean returns 0 having skipped a whole location while promising to remove everything; status returns 0 while having told the user nothing about a base it never examined. Decision: both must name the skipped target and exit 1.

### 33

the missing automated coverage is accepted. The four new tests cover only the clean-scanning helper; the exit-code branching, the confirmation choke point and the dry-run wording rest on one-off manual runs, one of which the implementer could not drive to completion. For a layer this thin those branches are unit testable without touching Steam or COM.

### 34

the stranded decision logic is accepted as Important, and the duplicated option string is the part that makes it urgent. The organization Select's third label is written twice — once in the option list, once in the match arm — with a catch-all default. Edit one and not the other and the match silently classifies "no folder" as "unified": no compile error, no failing test, and the user who picks "place them directly" gets a folder. It sits in the half nobody can execute. Decision: eliminate the string matching rather than merely testing it. Pair each label with its value in one table used both to render the prompt and to interpret the answer, so there is a single source of truth and no catch-all to fall through.

**Cost if wrong:** a slightly more elaborate options structure.

### 35

the silent icon-chain loss is accepted and fixed now rather than deferred, against the reviewer's own suggestion that it could be a fast follow-up. If steam.games() fails, every legacy item loses its game lookup; combined with the verified real-world fact that all eight of the maintainer's .url files reference missing icons, the result is that migrate deletes the originals and writes iconless shortcuts while printing "Each will be replaced by a .lnk with a working icon". That is this tool reproducing, silently, the exact defect it exists to fix. The conversion is still a net improvement — the .lnk launches and cannot degrade further — so refusing outright would be disproportionate. Decision: warn loudly when the library cannot be read, and count items that ended up without an icon so the summary is honest rather than unconditionally optimistic.

**Cost if wrong:** one extra warning line and a counter.

### 36

both documentation inaccuracies accepted and to be fixed. status() calls build_desired unconditionally with no dry_run gate, and build_desired resolves icons, which can write a converted .ico into the cache — so the README's unqualified "without changing anything" for the one command advertised as inert is false. And the icon-cache caveat is presented as uniform across commands when it is true for sync, vacuous for clean, and false for migrate, whose convert_one returns before ever calling icon_for. The reviewer also correctly noted the implementer's evidence for that claim — an unchanged cache mtime — is equally consistent with a warm cache and does not distinguish per-command behaviour.

### 37

the brief's Step 5 required a real sync, confirming an icon and a launch, then a real clean. I explicitly forbade the implementer from running anything that modifies the maintainer's Desktop or Start Menu, so it followed my instruction over the plan's. That override stands — creating real shortcuts on someone's machine is their decision, not an agent's mid-task. But I never recorded the override, so the report legitimately reads as an unexplained gap. Decision: the report must record it as a deliberate controller-directed omission, and the real end-to-end run becomes a decision to put to the maintainer directly.

### 38

derive candidates from the base directory rather than from remembered names. sync, status and clean should scan the base plus one level of subdirectories, claiming only signed shortcuts. Ownership gates every deletion and overwrite, so a wider scan cannot endanger anything foreign — the same argument clean_scan_dirs already makes. This handles any folder name the user has ever used, including one whose config entry was lost, which name-remembering cannot.

**Cost if wrong:** one extra readdir per base plus one per subdirectory.
