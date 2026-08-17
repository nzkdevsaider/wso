use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum VdfError {
    UnexpectedEnd,
    UnexpectedToken(String),
    NestedTooDeep,
}

impl fmt::Display for VdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VdfError::UnexpectedEnd => write!(f, "unexpected end of VDF input"),
            VdfError::UnexpectedToken(t) => write!(f, "unexpected token in VDF: {t}"),
            VdfError::NestedTooDeep => write!(f, "VDF nesting exceeded maximum depth"),
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
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let entries = p.parse_entries(true)?;
    Ok(Value::Obj(entries))
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

const MAX_NESTING_DEPTH: usize = 64;

fn utf8_width(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1, // continuation or invalid lead; the from_utf8 below rejects it
    }
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
                            if self.depth >= MAX_NESTING_DEPTH {
                                return Err(VdfError::NestedTooDeep);
                            }
                            self.depth += 1;
                            let obj = self.parse_entries(false)?;
                            self.depth -= 1;
                            entries.push((key, Value::Obj(obj)));
                        }
                        None => return Err(VdfError::UnexpectedEnd),
                        Some(c) => return Err(VdfError::UnexpectedToken((c as char).to_string())),
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
                        Some(b'n') => {
                            out.push('\n');
                            self.pos += 1;
                        }
                        Some(b't') => {
                            out.push('\t');
                            self.pos += 1;
                        }
                        Some(_) => {
                            // Decode escaped non-ASCII characters as UTF-8.
                            let lead = self.bytes[self.pos];
                            let width = utf8_width(lead);
                            let end = std::cmp::min(self.pos + width, self.bytes.len());
                            let window = &self.bytes[self.pos..end];
                            let s =
                                std::str::from_utf8(window).map_err(|_| VdfError::UnexpectedEnd)?;
                            let ch = s.chars().next().ok_or(VdfError::UnexpectedEnd)?;
                            out.push(ch);
                            self.pos += ch.len_utf8();
                        }
                    }
                }
                Some(lead) => {
                    // Advance one full UTF-8 character so non-ASCII titles survive.
                    let width = utf8_width(lead);
                    let end = std::cmp::min(self.pos + width, self.bytes.len());
                    let window = &self.bytes[self.pos..end];
                    let s = std::str::from_utf8(window).map_err(|_| VdfError::UnexpectedEnd)?;
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
        assert_eq!(
            root.get("1").unwrap().get("apps").unwrap().entries().len(),
            2
        );
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
        assert!(matches!(
            parse("\"root\"\n{\n"),
            Err(VdfError::UnexpectedEnd)
        ));
    }

    #[test]
    fn reports_unterminated_string() {
        assert!(matches!(parse("\"root"), Err(VdfError::UnexpectedEnd)));
    }

    #[test]
    fn parses_non_ascii_title_overwatch() {
        let v = parse("\"AppState\"\n{\n\"name\"\t\"Overwatch®\"\n}\n").unwrap();
        let state = v.get("AppState").unwrap();
        assert_eq!(state.get("name").unwrap().as_str(), Some("Overwatch®"));
    }

    #[test]
    fn parses_non_ascii_title_helldivers() {
        let v = parse("\"AppState\"\n{\n\"name\"\t\"HELLDIVERS™ 2\"\n}\n").unwrap();
        let state = v.get("AppState").unwrap();
        assert_eq!(state.get("name").unwrap().as_str(), Some("HELLDIVERS™ 2"));
    }

    #[test]
    fn parses_escaped_non_ascii_character() {
        // Build input string with an actual backslash before the UTF-8 character.
        // This tests the escape branch's UTF-8 handling.
        let mut input = String::from("\"AppState\"\n{\n\"name\"\t\"Caf");
        input.push('\\'); // Add the backslash that the parser will encounter
        input.push_str("é\"\n}\n"); // Add the multi-byte character
        let v = parse(&input).unwrap();
        let state = v.get("AppState").unwrap();
        // The escape branch decodes the multi-byte character, consuming the backslash.
        assert_eq!(state.get("name").unwrap().as_str(), Some("Café"));
    }

    #[test]
    fn rejects_excessive_nesting_depth() {
        let mut deep = String::from("\"root\"\n{\n");
        for _ in 0..70 {
            deep.push_str("\"k\"\n{\n");
        }
        assert!(matches!(parse(&deep), Err(VdfError::NestedTooDeep)));
    }

    /// The nesting cap must count how deep the parser currently *is*, not how
    /// many blocks it has seen — so `self.depth` has to come back down when a
    /// block closes. Steam's own `libraryfolders.vdf` is a flat list of
    /// sibling blocks, and a library with more than `MAX_NESTING_DEPTH`
    /// entries is entirely ordinary; without the decrement every such file
    /// would be rejected as "nested too deep" and the user's whole Steam
    /// library would be reported unreadable.
    ///
    /// The previous version of this test nested three siblings at depth 2
    /// against a cap of 64. Deleting `self.depth -= 1` would have left it
    /// passing comfortably — the fourth sibling would have been the first to
    /// notice, and there was no fourth. This one puts `MAX_NESTING_DEPTH + 1`
    /// siblings in a row, which is exactly the point where an accumulating
    /// counter first trips.
    #[test]
    fn sibling_blocks_do_not_accumulate_depth_even_past_the_nesting_cap() {
        let count = MAX_NESTING_DEPTH + 1;
        let mut input = String::from("\"libraryfolders\"\n{\n");
        for i in 0..count {
            input.push_str(&format!("\"{i}\" {{ \"path\" \"D:\\\\Library{i}\" }}\n"));
        }
        input.push_str("}\n");

        let v = parse(&input).expect("sibling blocks are one deep each, not cumulatively deep");
        let root = v.get("libraryfolders").unwrap();
        assert_eq!(root.entries().len(), count);
        assert_eq!(
            root.get(&(count - 1).to_string())
                .unwrap()
                .get("path")
                .unwrap()
                .as_str(),
            Some(&*format!("D:\\Library{}", count - 1)),
            "the last sibling, the one an accumulating depth counter would reject"
        );
    }

    /// The guard on the test above: the cap must still be a cap. These blocks
    /// are genuinely nested inside one another rather than sitting side by
    /// side, so a parser that simply stopped counting would accept them.
    #[test]
    fn genuinely_nested_blocks_are_still_rejected_one_past_the_cap() {
        let mut deep = String::from("\"root\"\n{\n");
        for _ in 0..MAX_NESTING_DEPTH {
            deep.push_str("\"k\"\n{\n");
        }
        assert!(matches!(parse(&deep), Err(VdfError::NestedTooDeep)));
    }

    /// And the other side of that boundary: nesting exactly to the cap must
    /// be accepted, so the limit is off-by-one-proof rather than merely
    /// "rejects something eventually".
    #[test]
    fn nesting_exactly_to_the_cap_is_accepted() {
        let mut deep = String::new();
        for i in 0..MAX_NESTING_DEPTH {
            deep.push_str(&format!("\"k{i}\"\n{{\n"));
        }
        deep.push_str("\"leaf\" \"value\"\n");
        for _ in 0..MAX_NESTING_DEPTH {
            deep.push_str("}\n");
        }
        assert!(
            parse(&deep).is_ok(),
            "nesting to exactly the cap must parse"
        );
    }
}
