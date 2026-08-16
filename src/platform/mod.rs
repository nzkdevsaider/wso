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
