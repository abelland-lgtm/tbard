//! Library sections (sidebar) and fuzzy search.

use crate::{db::Book, i18n::tr};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Filter {
    All,
    InProgress,
    Downloaded,
    NotDownloaded,
    Finished,
}

impl Filter {
    /// Sidebar sections, in order (keys 1–5).
    pub const ALL: [Self; 5] = [Self::All, Self::InProgress, Self::Downloaded, Self::NotDownloaded, Self::Finished];

    pub fn next(self) -> Self {
        match self {
            Self::All => Self::InProgress,
            Self::InProgress => Self::Downloaded,
            Self::Downloaded => Self::NotDownloaded,
            Self::NotDownloaded => Self::Finished,
            Self::Finished => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => tr("Library", "Biblioteca"),
            Self::InProgress => tr("In progress", "En curso"),
            Self::Downloaded => tr("Downloaded", "Descargados"),
            Self::NotDownloaded => tr("In the cloud", "Sin descargar"),
            Self::Finished => tr("Finished", "Terminados"),
        }
    }

    pub fn count(self, books: &[Book]) -> usize {
        books.iter().filter(|b| self.matches(b)).count()
    }

    fn matches(self, b: &Book) -> bool {
        match self {
            Self::All => true,
            Self::InProgress => b.in_progress(),
            Self::Downloaded => b.path.is_some(),
            Self::NotDownloaded => b.path.is_none(),
            Self::Finished => b.finished(),
        }
    }
}

/// Indexes of the visible books. Without a query the database order is kept
/// (most recently listened first); with one, by relevance.
pub fn visible(books: &[Book], filter: Filter, query: &str) -> Vec<usize> {
    let candidates = books.iter().enumerate().filter(|(_, b)| filter.matches(b));
    if query.is_empty() {
        return candidates.map(|(i, _)| i).collect();
    }
    let m = SkimMatcherV2::default();
    let mut scored: Vec<_> = candidates
        .filter_map(|(i, b)| m.fuzzy_match(&format!("{} {} {}", b.title, b.authors, b.series), query).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(title: &str, authors: &str, path: bool, percent: f64) -> Book {
        Book {
            asin: title.into(),
            title: title.into(),
            authors: authors.into(),
            series: String::new(),
            runtime_ms: 0,
            percent_complete: percent,
            cover_url: None,
            path: path.then(|| "/x".into()),
            acr: None,
            content_format: None,
            position_ms: None,
        }
    }

    #[test]
    fn filters_and_fuzzy_search() {
        let books = [
            book("Juramentada", "Brandon Sanderson", true, 40.0),
            book("Robot salvaje", "Peter Brown", false, 0.0),
            book("Amanecer Rojo", "Pierce Brown", false, 100.0),
        ];
        assert_eq!(visible(&books, Filter::All, ""), [0, 1, 2]);
        assert_eq!(visible(&books, Filter::InProgress, ""), [0]);
        assert_eq!(visible(&books, Filter::Downloaded, ""), [0]);
        assert_eq!(visible(&books, Filter::NotDownloaded, ""), [1, 2]);
        assert_eq!(visible(&books, Filter::Finished, ""), [2]);
        assert_eq!(visible(&books, Filter::All, "jrmnt"), [0]);
        assert_eq!(visible(&books, Filter::All, "brown"), [1, 2]);
        assert_eq!(visible(&books, Filter::Finished, "brown"), [2]);
    }
}
