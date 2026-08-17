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
        let title: String = std::iter::repeat('🎮').take(200).collect();
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
