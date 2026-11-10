//! md-doc-search library: offline full-text search over Markdown corpora,
//! tuned for grounding AI coding agents on exact documentation.
//!
//! The pipeline has three stages:
//!
//! - [`corpus`] loads a Markdown file, or every `.md` page under a directory
//!   (front matter stripped, canonical `url:` kept);
//! - [`parse`] slices a corpus into [`Section`]s along ATX/Setext headings
//!   (levels 1-3), skipping fenced code blocks and keeping each section's
//!   heading ancestry;
//! - [`rank`] scores sections against a keyword query (IDF weighting, heading
//!   bonuses, coverage) and returns the top-k [`Hit`]s;
//! - [`render`] lays the winning sections out under a character budget.
//!
//! [`search_markdown`] chains the three; errors come back as
//! [`SearchError`] values instead of being embedded in the output.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod corpus;
pub mod parse;
pub mod rank;
pub mod render;

pub use corpus::Source;
pub use parse::Section;
pub use rank::{Hit, RankingParams};
pub use render::render_hits;

use std::fmt;
use std::path::Path;

/// Errors returned by the search pipeline.
#[derive(Debug)]
pub enum SearchError {
    /// The corpus file or directory could not be read.
    Io(std::io::Error),
    /// The corpus directory holds no `.md` file.
    NoMarkdown(String),
    /// The query contains no usable keyword.
    EmptyQuery,
    /// A numeric argument is out of range (must be at least 1).
    InvalidParam(&'static str),
    /// A numeric argument is too large to be usable (overflows internal
    /// arithmetic).
    TooLarge(&'static str),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::Io(e) => write!(f, "cannot read corpus file: {e}"),
            SearchError::NoMarkdown(dir) => write!(f, "no .md file found under {dir}"),
            SearchError::EmptyQuery => write!(f, "query contains no keyword"),
            SearchError::InvalidParam(what) => write!(f, "{what} must be at least 1"),
            SearchError::TooLarge(what) => write!(f, "{what} is too large"),
        }
    }
}

impl std::error::Error for SearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SearchError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SearchError {
    fn from(e: std::io::Error) -> Self {
        SearchError::Io(e)
    }
}

/// Everything needed to turn a query into rendered output.
pub struct SearchOutcome {
    /// Full corpus text (sections index into it).
    pub corpus: String,
    /// Corpus sections, in document order.
    pub sections: Vec<Section>,
    /// Pages of a directory corpus, in corpus order (empty for a single file).
    pub sources: Vec<Source>,
    /// Top-k hits, best first (deterministic: score desc, then document order).
    pub hits: Vec<Hit>,
}

/// Load a corpus (a Markdown file or a directory of `.md` pages), split it
/// into sections and rank them against `query`.
///
/// Use [`render_hits`] on the returned outcome to build the final text.
///
/// # Errors
///
/// [`SearchError::InvalidParam`] when `top_k` is 0, [`SearchError::Io`] when
/// the corpus cannot be read, [`SearchError::NoMarkdown`] when a corpus
/// directory holds no `.md` file, [`SearchError::EmptyQuery`] when the query
/// holds no keyword.
pub fn search_markdown(
    corpus_path: &Path,
    query: &str,
    top_k: usize,
    params: &RankingParams,
) -> Result<SearchOutcome, SearchError> {
    if top_k == 0 {
        return Err(SearchError::InvalidParam("top_k"));
    }
    let loaded = corpus::load(corpus_path)?;
    let hits = rank::rank_sections(&loaded.text, &loaded.sections, query, top_k, params)?;
    Ok(SearchOutcome {
        corpus: loaded.text,
        sections: loaded.sections,
        sources: loaded.sources,
        hits,
    })
}
