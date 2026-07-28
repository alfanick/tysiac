//! Compact English, German, and Polish UI strings.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locale {
    English,
    German,
    Polish,
}

impl Locale {
    #[must_use]
    pub fn from_language_tag(tag: &str) -> Self {
        if tag.to_ascii_lowercase().starts_with("de") {
            Self::German
        } else if tag.to_ascii_lowercase().starts_with("pl") {
            Self::Polish
        } else {
            Self::English
        }
    }
}

#[must_use]
pub fn text(locale: Locale, key: &str) -> &str {
    match (locale, key) {
        (Locale::German, "join") => "Beitreten",
        (Locale::Polish, "join") => "Dołącz",
        (_, "join") => "Join",
        (Locale::German, "pass") => "Passen",
        (Locale::Polish, "pass") => "Pas",
        (_, "pass") => "Pass",
        (Locale::German, "play") => "Spielen",
        (Locale::Polish, "play") => "Zagraj",
        (_, "play") => "Play",
        (Locale::German, "observe") => "Zuschauen",
        (Locale::Polish, "observe") => "Obserwuj",
        (_, "observe") => "Observe",
        _ => key,
    }
}
