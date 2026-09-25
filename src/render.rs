//! Output building: greedy character budget, line-boundary truncation and
//! fence closing.
//!
//! The markers are kept in French for compatibility with existing `SKILL.md`
//! files and documented examples.

use std::borrow::Cow;

use crate::parse::{fence_closer, fence_opener};
use crate::{Hit, SearchError, SearchOutcome};

const END_MARKER: &str = "--- FIN DE SECTION ---";
const TRUNCATION_MARKER: &str = "\n\n[... Section tronquée pour respecter le budget de tokens ...]";

/// Render the ranked hits under the token budget (1 token ≈ 4 characters,
/// counted in characters, not bytes).
///
/// # Errors
///
/// [`SearchError::InvalidParam`] when `max_tokens` is 0.
pub fn render_hits(
    outcome: &SearchOutcome,
    query: &str,
    max_tokens: usize,
) -> Result<String, SearchError> {
    if max_tokens == 0 {
        return Err(SearchError::InvalidParam("max_tokens"));
    }
    let mut out = format!("Résultats de recherche pour '{query}' :\n\n");
    let mut remaining = (max_tokens * 4).saturating_sub(out.chars().count());

    for (rank, hit) in outcome.hits.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        // Greedy waterfall: each hit may spend an even share of what is left;
        // whatever a short section leaves unused flows to the next ones.
        let share = remaining / (outcome.hits.len() - rank);
        if share == 0 {
            break;
        }
        let block = render_block(outcome, hit, share);
        remaining = remaining.saturating_sub(block.chars().count());
        out.push_str(&block);
    }
    Ok(out)
}

fn render_block(outcome: &SearchOutcome, hit: &Hit, budget_chars: usize) -> String {
    let section = &outcome.sections[hit.section_idx];
    let body = &outcome.corpus[section.range.clone()];

    let score = hit.score.round();
    let mut block = format!("--- DÉBUT DE SECTION (Score: {score}) ---\n");
    if section.level > 0 {
        block.push_str(&"#".repeat(usize::from(section.level)));
        block.push(' ');
        block.push_str(&section.heading);
        block.push('\n');
    }
    if !section.breadcrumb.is_empty() {
        block.push_str("> ");
        block.push_str(&section.breadcrumb.join(" > "));
        block.push('\n');
    }

    // Reserve room for the markers so the whole block stays within budget.
    let overhead = block.chars().count() + END_MARKER.chars().count() + 3;
    let body_budget = budget_chars.saturating_sub(overhead);
    let (text, truncated) = if body.chars().count() <= body_budget {
        (Cow::Borrowed(body), false)
    } else {
        let cut_budget = body_budget.saturating_sub(TRUNCATION_MARKER.chars().count());
        if cut_budget == 0 {
            (Cow::Borrowed(""), false)
        } else {
            (truncate_body(body, cut_budget), true)
        }
    };
    block.push_str(&text);
    if truncated {
        block.push_str(TRUNCATION_MARKER);
    }
    block.push('\n');
    block.push_str(END_MARKER);
    block.push_str("\n\n");
    block
}

/// Cut `body` to at most `budget` characters, ending on a line boundary, and
/// close a code fence left open by the cut.
fn truncate_body(body: &str, budget: usize) -> Cow<'_, str> {
    let Some((byte_end, _)) = body.char_indices().nth(budget) else {
        return Cow::Borrowed(body); // fewer characters than the budget
    };
    let head = &body[..byte_end];
    let cut = match head.rfind('\n') {
        Some(pos) => &body[..=pos], // keep the newline: end of a complete line
        None => head,               // a single long line: hard cut at the char boundary
    };
    // Re-scan the kept lines for an unclosed fence.
    let mut fence = None;
    for line in cut.split_inclusive('\n') {
        match fence {
            Some(open) => {
                if fence_closer(line, open) {
                    fence = None;
                }
            }
            None => fence = fence_opener(line),
        }
    }
    match fence {
        None => Cow::Borrowed(cut),
        Some(open) => {
            let mut owned = String::from(cut);
            if !owned.ends_with('\n') {
                owned.push('\n');
            }
            owned.extend(std::iter::repeat_n(open.ch as char, open.len));
            Cow::Owned(owned)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_sections;
    use crate::rank::{RankingParams, rank_sections};

    fn outcome(corpus: &str, query: &str, top_k: usize) -> SearchOutcome {
        let sections = parse_sections(corpus);
        let hits =
            rank_sections(corpus, &sections, query, top_k, &RankingParams::default()).unwrap();
        SearchOutcome {
            corpus: corpus.to_string(),
            sections,
            hits,
        }
    }

    #[test]
    fn zero_max_tokens_is_an_error() {
        let o = outcome("# A\n\nbody\n", "body", 1);
        assert!(matches!(
            render_hits(&o, "body", 0),
            Err(SearchError::InvalidParam(_))
        ));
    }

    #[test]
    fn budget_is_respected_in_characters_not_bytes() {
        // 800 accented characters = 1600 bytes but 800 chars.
        let body = "é".repeat(800);
        let corpus = format!("# A\n\n{body}\n");
        let o = outcome(&corpus, "é", 1);
        let out = render_hits(&o, "é", 100).unwrap(); // budget: 400 chars
        assert!(out.chars().count() <= 400);
        assert!(out.contains("tronquée"));
    }

    #[test]
    fn short_first_section_gives_budget_to_the_next() {
        // A (short, ranks first) leaves almost all of its uniform share
        // unused; B (long, ranks second) must inherit it.
        let corpus = format!(
            "# A\n\nshort short short\n\n# B\n\n{}{}TAILMARK\n",
            "padding ".repeat(247),
            "word ".repeat(2)
        );
        let o = outcome(&corpus, "short word", 2);
        // 600 tokens = 2400 chars: a uniform split (~1178 each) would
        // truncate B (~2060 needed); the waterfall does not.
        let out = render_hits(&o, "short word", 600).unwrap();
        assert!(!out.contains("tronquée"));
        assert!(out.contains("TAILMARK"));
    }

    #[test]
    fn truncation_lands_on_line_boundary() {
        let line = "abc".repeat(20); // 60 chars
        let corpus = format!("# A\n\n{}\n", vec![line; 30].join("\n"));
        let o = outcome(&corpus, "abc", 1);
        let out = render_hits(&o, "abc", 100).unwrap();
        assert!(out.contains("tronquée"));
        // Every kept body line is a complete 60-char line, never a partial one.
        assert!(out.lines().all(|l| l.len() == 60 || !l.starts_with("abc")));
    }

    #[test]
    fn open_fence_is_closed_after_truncation() {
        let mut body = String::from("intro here\n\n```python\n");
        for i in 0..40 {
            body.push_str(&format!("code_line_{i} = {i}\n"));
        }
        let corpus = format!("# A\n\n{body}");
        let o = outcome(&corpus, "code_line", 1);
        let out = render_hits(&o, "code_line", 60).unwrap();
        let (_before, rest) = out.split_once("```python").unwrap();
        let (kept, _after) = rest.split_once("[... Section tronquée").unwrap();
        assert!(kept.trim_end().ends_with("```"));
    }

    #[test]
    fn preamble_hit_has_no_heading_line() {
        let corpus = "Preamble text about widgets.\n\n# Page\n\nwidget body\n";
        let o = outcome(corpus, "preamble", 1);
        let out = render_hits(&o, "preamble", 100).unwrap();
        let (_head, rest) = out.split_once("--- DÉBUT DE SECTION").unwrap();
        let (kept, _tail) = rest.split_once("--- FIN DE SECTION").unwrap();
        let lines: Vec<&str> = kept.lines().collect();
        assert_eq!(lines[1], "Preamble text about widgets.");
    }
}
