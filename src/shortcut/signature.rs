use crate::platform::Platform;
use crate::shortcut::writer::DESCRIPTION_LIMIT_UTF16;

const PREFIX: &str = "[wso:";
const SUFFIX: char = ']';

/// Appended when a title is truncated to make room for the marker. A
/// single BMP character, so it costs exactly one UTF-16 unit — cheap
/// enough to always afford once truncation is already happening.
const ELLIPSIS: char = '…';

/// Identifies a shortcut this tool owns. Stored in the `.lnk` Description
/// field, which Windows shows as the hover tooltip, so it is written to be
/// readable by humans as well as by us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub platform: Platform,
    pub id: String,
}

/// Builds the description stored in a shortcut's `.lnk`. A Windows shell
/// link's description cannot survive a save/load cycle past
/// `DESCRIPTION_LIMIT_UTF16` (260) UTF-16 code units — see
/// `shortcut::writer` for how that was measured — and `write_lnk` rejects
/// anything longer outright rather than let Windows truncate it silently.
///
/// The `[wso:{platform}:{id}]` marker at the end is the ownership
/// guarantee this whole tool depends on: `clean` and `sync` both use it to
/// recognize a shortcut they created. Losing it is far worse than losing
/// some tooltip text — `clean` would abandon the shortcut forever and the
/// next `sync` would create a duplicate beside it. So when the display
/// name plus marker would exceed the limit, the display name gives way:
/// it is truncated (to a whole character, never a split surrogate pair,
/// and with a trailing `…` when there's room for one) so the marker always
/// survives intact.
pub fn encode(display_name: &str, sig: &Signature) -> String {
    let suffix = format!(
        " — managed by WinShortcutsOrganizer [wso:{}:{}]",
        sig.platform.as_str(),
        sig.id
    );
    let budget = DESCRIPTION_LIMIT_UTF16.saturating_sub(suffix.encode_utf16().count());
    let title = truncate_to_utf16_budget(display_name, budget);
    format!("{title}{suffix}")
}

/// Returns `value` unchanged if it already fits in `budget` UTF-16 code
/// units. Otherwise truncates to the longest prefix of whole characters
/// that fits — never splitting a multi-unit character (a surrogate pair,
/// in UTF-16 terms) — and appends `ELLIPSIS` if the budget has room left
/// over for it once truncation is already happening.
///
/// Length is counted via `char::len_utf16()`, the same unit Windows'
/// shell-link API measures strings in: characters outside the Basic
/// Multilingual Plane (emoji included — these appear in real Steam
/// titles) cost two units, not the one a `.chars().count()` budget would
/// assume.
fn truncate_to_utf16_budget(value: &str, budget: usize) -> String {
    if value.encode_utf16().count() <= budget {
        return value.to_string();
    }

    let content_budget = budget.saturating_sub(ELLIPSIS.len_utf16());
    let mut used = 0usize;
    let mut end_byte = 0usize;
    for c in value.chars() {
        let c_len = c.len_utf16();
        if used + c_len > content_budget {
            break;
        }
        used += c_len;
        end_byte += c.len_utf8();
    }

    let mut truncated = value[..end_byte].to_string();
    if budget >= ELLIPSIS.len_utf16() {
        truncated.push(ELLIPSIS);
    }
    truncated
}

/// Whether `description` presents itself as one of ours — it contains the
/// `[wso:` that opens an ownership marker and ends with the `]` that closes
/// one — while [`decode`] cannot actually recover a signature from it.
///
/// This is the one shape a shortcut must never be written in, and nothing
/// upstream prevents it: [`encode`] performs no validation of its own, while
/// `decode` rejects an id containing `:`, `[` or `]`, and Epic's id is the
/// manifest's `AppName` verbatim (`platform::epic`). An `AppName` carrying
/// any of those three characters would produce a perfectly valid `.lnk`
/// that `ops::scan_owned` can never claim back: `clean` could not remove it,
/// and every later `sync` would create a duplicate beside it, forever, with
/// no way for the user to stop it short of deleting the file by hand.
///
/// Defined by running `decode` rather than by restating its rules, so the
/// two can never drift apart. `writer::write_lnk` refuses such a
/// description before touching the filesystem, which turns an
/// unmanageable-forever shortcut into an ordinary reported per-item failure.
pub fn claims_ownership_but_does_not_decode(description: &str) -> bool {
    let trimmed = description.trim_end();
    trimmed.ends_with(SUFFIX) && trimmed.contains(PREFIX) && decode(trimmed).is_none()
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
    if id.is_empty() || id.contains(':') || id.contains('[') || id.contains(']') {
        return None;
    }

    Some(Signature {
        platform: Platform::from_str_tag(tag)?,
        id: id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    #[test]
    fn round_trips() {
        let sig = Signature {
            platform: Platform::Steam,
            id: "105600".into(),
        };
        let desc = encode("Terraria", &sig);
        assert_eq!(
            desc,
            "Terraria — managed by WinShortcutsOrganizer [wso:steam:105600]"
        );
        assert_eq!(decode(&desc), Some(sig));
    }

    #[test]
    fn decodes_epic_alphanumeric_ids() {
        let sig = Signature {
            platform: Platform::Epic,
            id: "Fortnite".into(),
        };
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
        assert_eq!(decode("Cool Game [wso:steam:105600] notes here]"), None);
        assert_eq!(decode("Something [wso:steam:12]]"), None);
    }

    /// Ids chosen to attack the marker format itself. Steam appids are
    /// numeric, but Epic's id is the manifest's `AppName` verbatim
    /// (`platform::epic`) — an arbitrary vendor-supplied string this crate
    /// never inspects — so the interesting cases are the three characters
    /// `decode` treats structurally (`:`, `[`, `]`), the empty id it also
    /// rejects, and a whole marker smuggled inside an id.
    const ADVERSARIAL_IDS: &[&str] = &[
        "440",
        "Fortnite",
        "Fall-Guys_1.2",
        "0a2d9f",
        "",
        "Game:Name",
        "a:b:c",
        "Game[Name",
        "Game]Name",
        "]",
        "[",
        "[wso:steam:440]",
        "440] and more",
        "with space",
        "🎮",
        "…",
    ];

    /// The property that matters is not "every id round-trips" — three
    /// characters genuinely cannot survive this marker format — but that
    /// there is no third outcome. Either `decode` recovers exactly the
    /// signature `encode` was given, or the description is one
    /// `writer::write_lnk` refuses outright. What must never happen is a
    /// `.lnk` reaching disk that `ops::scan_owned` cannot claim back:
    /// `clean` could never remove it, and every later `sync` would create a
    /// duplicate beside it, forever.
    ///
    /// `encode` performed no validation at all before this guard existed, so
    /// an Epic `AppName` containing a colon produced exactly that shortcut,
    /// silently.
    #[test]
    fn every_id_either_round_trips_exactly_or_is_refused_before_it_can_be_written() {
        for id in ADVERSARIAL_IDS {
            for platform in [Platform::Steam, Platform::Epic] {
                let sig = Signature {
                    platform,
                    id: (*id).to_string(),
                };
                let desc = encode("Some Game", &sig);

                match decode(&desc) {
                    Some(recovered) => {
                        assert_eq!(
                            recovered, sig,
                            "id {id:?} decoded, but not back to the signature it was encoded \
                             from; description was {desc:?}"
                        );
                        assert!(
                            !claims_ownership_but_does_not_decode(&desc),
                            "id {id:?} round-trips, so it must not be refused: {desc:?}"
                        );
                    }
                    None => assert!(
                        claims_ownership_but_does_not_decode(&desc),
                        "id {id:?} produced a description that cannot be decoded yet would not \
                         be refused: {desc:?} — this is the shortcut that can never be claimed \
                         back"
                    ),
                }
            }
        }
    }

    /// At least one adversarial id must actually take the refused branch
    /// above, or the property test would pass just as happily against a
    /// corpus of well-behaved ids and prove nothing about the guard.
    #[test]
    fn a_colon_in_an_id_is_refused_rather_than_silently_encoded() {
        let sig = Signature {
            platform: Platform::Epic,
            id: "Fall:Guys".into(),
        };
        let desc = encode("Fall Guys", &sig);
        assert_eq!(decode(&desc), None, "sanity: this id cannot round-trip");
        assert!(claims_ownership_but_does_not_decode(&desc), "{desc:?}");
    }

    /// The guard must not fire on an ordinary foreign description that never
    /// claimed to be ours in the first place — that would turn `write_lnk`
    /// into something that refuses perfectly good work.
    #[test]
    fn a_description_with_no_marker_at_all_is_not_treated_as_a_broken_marker() {
        assert!(!claims_ownership_but_does_not_decode("My own shortcut"));
        assert!(!claims_ownership_but_does_not_decode(""));
        assert!(!claims_ownership_but_does_not_decode(
            "Team Fortress 2 [Beta]"
        ));
    }

    #[test]
    fn ignores_brackets_earlier_in_the_name() {
        let sig = Signature {
            platform: Platform::Steam,
            id: "440".into(),
        };
        let desc = encode("Team Fortress 2 [Beta]", &sig);
        assert_eq!(decode(&desc), Some(sig));
    }

    /// Truncation must only engage when the title actually needs it — an
    /// ordinary short title must come out exactly as given, byte-for-byte,
    /// not merely "close enough" or "under the limit by some margin".
    #[test]
    fn does_not_truncate_a_title_that_already_fits() {
        let sig = Signature {
            platform: Platform::Steam,
            id: "105600".into(),
        };
        let desc = encode("Terraria", &sig);
        assert_eq!(
            desc,
            "Terraria — managed by WinShortcutsOrganizer [wso:steam:105600]"
        );
    }

    /// Steam titles do contain characters outside the Basic Multilingual
    /// Plane (emoji included), and each of those costs two UTF-16 units,
    /// not one. A budget computed from `.chars().count()` instead of
    /// UTF-16 units would let a non-BMP-heavy title overrun the true
    /// 260-unit shell-link limit. This also proves the cut point never
    /// lands inside a surrogate pair: if it did, `String::from_utf16`
    /// applied to the description's own UTF-16 units would fail, because a
    /// lone surrogate is not valid UTF-16.
    #[test]
    fn truncates_a_title_padded_with_non_bmp_characters_without_splitting_a_surrogate_pair() {
        let sig = Signature {
            platform: Platform::Steam,
            id: "105600".into(),
        };
        // U+1F3AE VIDEO GAME — two UTF-16 units per character. 200 copies
        // is far past any plausible budget under the 260-unit limit.
        let title: String = "🎮".repeat(200);
        let desc = encode(&title, &sig);

        let unit_count = desc.encode_utf16().count();
        assert!(
            unit_count <= 260,
            "description must fit the measured shell-link limit, was {unit_count}"
        );

        let units: Vec<u16> = desc.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            desc,
            "truncation must never split a surrogate pair"
        );

        assert_eq!(decode(&desc), Some(sig));
    }
}
