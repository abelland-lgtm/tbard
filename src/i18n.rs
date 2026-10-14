//! Interface language. Texts are written in place as `tr("English", "Español")`
//! instead of keys into a catalog: with two languages that keeps each string
//! next to the code that shows it.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lang {
    En,
    Es,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Es];

    /// The value stored in config.toml.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
        }
    }

    pub fn parse(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code() == code)
    }

    /// The language's own name, for the picker.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Es => "Español",
        }
    }

    /// From a locale tag such as `es_CO.UTF-8` or `en-US`; English otherwise.
    pub fn from_locale(tag: &str) -> Lang {
        if tag.to_ascii_lowercase().starts_with("es") {
            Lang::Es
        } else {
            Lang::En
        }
    }

    /// The operating system's language.
    pub fn system() -> Lang {
        sys_locale::get_locale().map_or(Lang::En, |l| Lang::from_locale(&l))
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == Lang::Es as u8 {
        Lang::Es
    } else {
        Lang::En
    }
}

/// The text in the current language.
pub fn tr(en: &'static str, es: &'static str) -> &'static str {
    match get() {
        Lang::En => en,
        Lang::Es => es,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_detection() {
        assert_eq!(Lang::from_locale("es_CO.UTF-8"), Lang::Es);
        assert_eq!(Lang::from_locale("es-419"), Lang::Es);
        assert_eq!(Lang::from_locale("en-US"), Lang::En);
        assert_eq!(Lang::from_locale("C"), Lang::En);
        assert_eq!(Lang::parse("es"), Some(Lang::Es));
        assert_eq!(Lang::parse("fr"), None);
    }
}
