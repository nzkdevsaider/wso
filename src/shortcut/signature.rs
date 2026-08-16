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
        assert_eq!(decode("Cool Game [wso:steam:105600] notes here]"), None);
        assert_eq!(decode("Something [wso:steam:12]]"), None);
    }

    #[test]
    fn ignores_brackets_earlier_in_the_name() {
        let sig = Signature { platform: Platform::Steam, id: "440".into() };
        let desc = encode("Team Fortress 2 [Beta]", &sig);
        assert_eq!(decode(&desc), Some(sig));
    }
}
