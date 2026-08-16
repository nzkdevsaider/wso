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
