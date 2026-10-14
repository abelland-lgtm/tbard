use ratatui::style::Color;
use serde::Deserialize;
use std::{path::Path, str::FromStr};

/// Colores de la TUI. Se leen de `~/.config/tbard/colors.toml` (lo genera la
/// plantilla de Noctalia); lo que falte usa los colores de la terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub primary: Color,
    pub on_primary: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub error: Color,
    pub surface: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub outline: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            primary: Color::Cyan,
            on_primary: Color::Black,
            secondary: Color::Magenta,
            tertiary: Color::Yellow,
            error: Color::Red,
            surface: Color::Reset,
            on_surface: Color::Reset,
            on_surface_variant: Color::DarkGray,
            outline: Color::DarkGray,
        }
    }
}

#[derive(Deserialize, Default)]
struct File {
    primary: Option<String>,
    on_primary: Option<String>,
    secondary: Option<String>,
    tertiary: Option<String>,
    error: Option<String>,
    surface: Option<String>,
    on_surface: Option<String>,
    on_surface_variant: Option<String>,
    outline: Option<String>,
}

impl Theme {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    fn parse(toml_src: &str) -> Self {
        let f: File = toml::from_str(toml_src).unwrap_or_default();
        let d = Self::default();
        let c = |v: Option<String>, fallback: Color| v.and_then(|s| Color::from_str(&s).ok()).unwrap_or(fallback);
        Self {
            primary: c(f.primary, d.primary),
            on_primary: c(f.on_primary, d.on_primary),
            secondary: c(f.secondary, d.secondary),
            tertiary: c(f.tertiary, d.tertiary),
            error: c(f.error, d.error),
            surface: c(f.surface, d.surface),
            on_surface: c(f.on_surface, d.on_surface),
            on_surface_variant: c(f.on_surface_variant, d.on_surface_variant),
            outline: c(f.outline, d.outline),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_keeps_defaults() {
        let t = Theme::parse("primary = \"#a6e3a1\"\nerror = \"nope\"\n");
        assert_eq!(t.primary, Color::Rgb(0xa6, 0xe3, 0xa1));
        assert_eq!(t.error, Theme::default().error);
        assert_eq!(Theme::parse("no es toml ["), Theme::default());
    }
}
