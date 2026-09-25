//! CLI entry point.
//!
//! Exit codes follow grep conventions: 0 = results returned, 1 = no result,
//! 2 = error (details on stderr).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use md_doc_search::{RankingParams, SearchError, render_hits, search_markdown};

const DEFAULT_MAX_TOKENS: usize = 8_000;
const DEFAULT_TOP_K: usize = 3;

/// Offline full-text search over a Markdown corpus, tuned for AI coding agents.
#[derive(Parser)]
#[command(name = "md-doc-search", version)]
struct Cli {
    /// Path to the Markdown corpus to search
    corpus: PathBuf,

    /// Query keywords, as a single argument (spaces allowed)
    query: String,

    /// Output budget in tokens (1 token ≈ 4 chars) — positional form
    #[arg(value_name = "MAX_TOKENS")]
    max_tokens_positional: Option<usize>,

    /// Number of sections to return — positional form
    #[arg(value_name = "TOP_K")]
    top_k_positional: Option<usize>,

    /// Output budget in tokens (1 token ≈ 4 chars)
    #[arg(long, conflicts_with = "max_tokens_positional")]
    max_tokens: Option<usize>,

    /// Number of sections to return
    #[arg(long, conflicts_with = "top_k_positional")]
    top_k: Option<usize>,

    /// Fold accented characters (é -> e) before matching
    #[arg(long)]
    fold_diacritics: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let max_tokens = cli
        .max_tokens
        .or(cli.max_tokens_positional)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let top_k = cli.top_k.or(cli.top_k_positional).unwrap_or(DEFAULT_TOP_K);

    let params = RankingParams {
        fold_diacritics: cli.fold_diacritics,
        ..RankingParams::default()
    };

    let outcome = match search_markdown(&cli.corpus, &cli.query, top_k, &params) {
        Ok(outcome) => outcome,
        Err(e) => return fail(&e),
    };
    if outcome.hits.is_empty() {
        eprintln!("No section matches the query '{}'.", cli.query);
        return ExitCode::from(1);
    }
    match render_hits(&outcome, &cli.query, max_tokens) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn fail(e: &SearchError) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::from(2)
}
