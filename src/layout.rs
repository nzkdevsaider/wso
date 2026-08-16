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
