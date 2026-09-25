//! Keyword scoring: IDF weighting, heading/phrase/whole-word bonuses, coverage.

use std::collections::HashSet;

use crate::SearchError;
use crate::parse::Section;

/// Ranking knobs, tuned for technical documentation corpora. Override for
/// experimentation; [`RankingParams::default`] holds the shipped values.
#[derive(Debug, Clone)]
pub struct RankingParams {
    /// Floor for the IDF weight, so ubiquitous terms still count a little.
    pub idf_floor: f64,
    /// Max occurrences of one keyword taken into account per section.
    pub occ_cap: usize,
    /// Bonus when a keyword appears in the section's own heading.
    pub heading_bonus: f64,
    /// Bonus when a keyword appears in an ancestor heading (page title, ...).
    pub page_bonus: f64,
    /// Multiplier when *all* keywords appear in the heading chain.
    pub all_in_headings_mult: f64,
    /// Bonus for an exact-phrase match (multi-keyword queries only).
    pub phrase_bonus: f64,
    /// Length-normalisation scale, in body bytes.
    pub len_norm_bytes: f64,
    /// Bonus when a keyword matches as a whole word (not inside another word).
    pub whole_word_bonus: f64,
    /// Multiplier applied to sections without a body (bare headings). They
    /// stay findable — a word that only exists on such a heading must return
    /// a hit — but must not steal a top-k slot from content sections.
    pub empty_body_factor: f64,
    /// Bonus when an H1/H2 heading *starts with* the full query (word
    /// boundary), so a class page like `# StandardMaterial3D` outranks method
    /// sections of other classes that merely cite the type in their H3
    /// signature.
    pub title_prefix_bonus: f64,
    /// Fold accented characters (`é` -> `e`) before matching, for non-English
    /// corpora.
    pub fold_diacritics: bool,
}

impl Default for RankingParams {
    fn default() -> Self {
        Self {
            idf_floor: 0.25,
            occ_cap: 20,
            heading_bonus: 15.0,
            page_bonus: 10.0,
            all_in_headings_mult: 1.4,
            phrase_bonus: 15.0,
            len_norm_bytes: 6000.0,
            whole_word_bonus: 3.0,
            empty_body_factor: 0.2,
            title_prefix_bonus: 25.0,
            fold_diacritics: false,
        }
    }
}

/// A ranked section: score plus index into the section list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Relevance score, higher is better.
    pub score: f64,
    /// Index into the section list the score refers to.
    pub section_idx: usize,
}

fn idf_weight(df: usize, n_sections: usize, floor: f64) -> f64 {
    (((n_sections + 1) as f64) / ((df + 1) as f64))
        .ln()
        .max(floor)
}

/// Fold common Latin accents to their ASCII base letter. Input is expected
/// lowercase; ligatures collapse to their first letter.
pub(crate) fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'æ' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'œ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            'ñ' => 'n',
            'ç' => 'c',
            'ß' => 's',
            other => other,
        })
        .collect()
}

/// (substring occurrences, whole-word occurrences) of `needle` in `hay`.
///
/// A whole word is delimited by a text edge, a non-alphanumeric ASCII byte or
/// an underscore (so `action` matches inside `is_action_just_pressed`); any
/// non-ASCII byte counts as a boundary (approximation for accented letters).
fn count_matches(hay: &str, needle: &str) -> (usize, usize) {
    let bytes = hay.as_bytes();
    let mut occ = 0;
    let mut whole = 0;
    for (i, _) in hay.match_indices(needle) {
        occ += 1;
        let before = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let end = i + needle.len();
        let after = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before && after {
            whole += 1;
        }
    }
    (occ, whole)
}

/// Score every section against `query` and return the top-k hits, best first
/// (deterministic: score descending, then document order).
///
/// # Errors
///
/// [`SearchError::InvalidParam`] when `top_k` is 0, [`SearchError::EmptyQuery`]
/// when the query holds no keyword.
pub fn rank_sections(
    corpus: &str,
    sections: &[Section],
    query: &str,
    top_k: usize,
    params: &RankingParams,
) -> Result<Vec<Hit>, SearchError> {
    if top_k == 0 {
        return Err(SearchError::InvalidParam("top_k"));
    }

    // Normalize keywords once: lowercase, optional accent folding, dedupe
    // (query order preserved — it defines the exact phrase).
    let normalize = |s: &str| {
        let lower = s.to_lowercase();
        if params.fold_diacritics {
            fold(&lower)
        } else {
            lower
        }
    };
    let mut keywords: Vec<String> = query
        .split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    let mut seen = HashSet::with_capacity(keywords.len());
    keywords.retain(|kw| seen.insert(kw.clone()));
    if keywords.is_empty() {
        return Err(SearchError::EmptyQuery);
    }
    let k = keywords.len();
    let phrase = keywords.join(" ");

    // One lowercase (and optional fold) pass per section, shared by the
    // document-frequency count and the scoring loop.
    let bodies: Vec<String> = sections
        .iter()
        .map(|s| normalize(&corpus[s.range.clone()]))
        .collect();
    let headings: Vec<String> = sections.iter().map(|s| normalize(&s.heading)).collect();
    let crumbs: Vec<String> = sections
        .iter()
        .map(|s| normalize(&s.breadcrumb.join(" ")))
        .collect();

    // Occurrences of every keyword in every section (one scan per keyword).
    let n = sections.len();
    let mut occ = vec![0usize; n * k];
    let mut whole = vec![0usize; n * k];
    for (si, body) in bodies.iter().enumerate() {
        for (ki, kw) in keywords.iter().enumerate() {
            let (o, w) = count_matches(body, kw);
            occ[si * k + ki] = o.min(params.occ_cap);
            whole[si * k + ki] = w.min(params.occ_cap);
        }
    }

    // Document frequency of each keyword, from the occurrence table.
    let dfs: Vec<usize> = (0..k)
        .map(|ki| (0..n).filter(|&si| occ[si * k + ki] > 0).count())
        .collect();
    let weights: Vec<f64> = dfs
        .iter()
        .map(|&df| idf_weight(df, n, params.idf_floor))
        .collect();
    let avg_weight = weights.iter().sum::<f64>() / k as f64;

    let mut scored: Vec<(f64, usize)> = Vec::new();
    for si in 0..n {
        let head = &headings[si];
        let crumb = &crumbs[si];
        let body = &bodies[si];
        let mut score = 0.0f64;
        let mut matched = 0usize;
        for ki in 0..k {
            let kw = keywords[ki].as_str();
            let weight = weights[ki];
            let in_heading = head.contains(kw);
            let in_crumb = crumb.contains(kw);
            if occ[si * k + ki] > 0 || in_heading || in_crumb {
                matched += 1;
            }
            score += occ[si * k + ki] as f64 * weight;
            if whole[si * k + ki] > 0 {
                score += params.whole_word_bonus * weight;
            }
            if in_heading {
                score += params.heading_bonus * weight;
            }
            if in_crumb {
                score += params.page_bonus * weight;
            }
        }
        if score <= 0.0 {
            continue;
        }

        // All query terms somewhere in the heading chain: strong context signal.
        if keywords
            .iter()
            .all(|kw| head.contains(kw.as_str()) || crumb.contains(kw.as_str()))
        {
            score *= params.all_in_headings_mult;
        }
        // Exact phrase, built from normalized keywords so extra spaces or case
        // in the query don't defeat it. Skipped for single-keyword queries,
        // where it would double-count occurrences.
        if k > 1 && (head.contains(phrase.as_str()) || body.contains(phrase.as_str())) {
            score += params.phrase_bonus * avg_weight;
        }
        // Title-prefix bonus: an H1/H2 heading that starts with the first
        // keyword and contains every keyword is very likely *the* page about
        // the query (class pages, tutorial titles — including CamelCase
        // identifiers like `ResourcePreloader` for "resource preloader"),
        // even when its body is thin and other sections mention the terms
        // more often. H3+ signatures citing the terms do not qualify.
        if sections[si].level <= 2
            && head.starts_with(keywords[0].as_str())
            && keywords.iter().all(|kw| head.contains(kw.as_str()))
        {
            score += params.title_prefix_bonus * avg_weight;
        }
        // Coverage: a section must speak to the whole query to keep its full
        // score, so one repeated term cannot outrank a section covering them all.
        if matched < k {
            score *= matched as f64 / k as f64;
        }
        // Length normalization: giant sections cannot win by being giant.
        let body_bytes = sections[si].range.len();
        score /= 1.0 + (body_bytes as f64 / params.len_norm_bytes).sqrt();
        // Bare headings stay findable but cannot outrank content sections.
        if !sections[si].has_body {
            score *= params.empty_body_factor;
        }
        scored.push((score, si));
    }

    if scored.is_empty() {
        return Ok(Vec::new());
    }
    let kk = top_k.min(scored.len());
    scored.select_nth_unstable_by(kk - 1, hit_order);
    scored[..kk].sort_by(hit_order);
    Ok(scored[..kk]
        .iter()
        .map(|&(score, section_idx)| Hit { score, section_idx })
        .collect())
}

/// Descending score, then document order; `total_cmp` keeps this panic-free.
fn hit_order(a: &(f64, usize), b: &(f64, usize)) -> std::cmp::Ordering {
    b.0.total_cmp(&a.0).then(a.1.cmp(&b.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_sections;

    fn hits(corpus: &str, query: &str, top_k: usize) -> Vec<Hit> {
        let sections = parse_sections(corpus);
        rank_sections(corpus, &sections, query, top_k, &RankingParams::default()).unwrap()
    }

    #[test]
    fn empty_query_is_an_error() {
        let corpus = "# A\n\nbody\n";
        let sections = parse_sections(corpus);
        assert!(matches!(
            rank_sections(corpus, &sections, "   ", 3, &RankingParams::default()),
            Err(SearchError::EmptyQuery)
        ));
    }

    #[test]
    fn zero_top_k_is_an_error() {
        let corpus = "# A\n\nbody\n";
        let sections = parse_sections(corpus);
        assert!(matches!(
            rank_sections(corpus, &sections, "body", 0, &RankingParams::default()),
            Err(SearchError::InvalidParam(_))
        ));
    }

    #[test]
    fn coverage_prefers_sections_matching_all_terms() {
        // A repeats one term; B matches both (one partially); C matches both
        // with the exact phrase. Without the coverage factor, A would rank
        // above B on sheer occurrence count.
        let corpus = "# A\n\nalpha alpha alpha alpha alpha alpha\n\n# B\n\nalpha xbeta\n\n# C\n\nalpha beta\n";
        let h = hits(corpus, "alpha beta", 3);
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].section_idx, 2);
        assert_eq!(h[1].section_idx, 1);
        assert!(h[1].score > h[2].score);
    }

    #[test]
    fn phrase_ignores_extra_whitespace_in_query() {
        // Both sections match the keywords with equal weight; only B contains
        // the exact normalized phrase, so only a whitespace-insensitive phrase
        // match can put B first.
        let corpus = "# A\n\nvolume and scatter mix here\n\n# B\n\nvolume scatter works\n";
        let h = hits(corpus, "Volume    Scatter", 2);
        assert_eq!(h[0].section_idx, 1);
    }

    #[test]
    fn occurrence_cap_flattens_repetition() {
        let long = "alpha ".repeat(30);
        let short = "alpha ".repeat(20);
        // Same body byte length (including the trailing newline), one with 30
        // occurrences (capped) and one with 20 plus inert padding: identical
        // scores.
        let pad_len = long.trim_end().len() - short.trim_end().len();
        let corpus = format!(
            "# A\n\n{}\n\n# B\n\n{} {}\n",
            long.trim_end(),
            short.trim_end(),
            "z".repeat(pad_len)
        );
        let h = hits(&corpus, "alpha", 2);
        assert_eq!(h.len(), 2);
        assert!((h[0].score - h[1].score).abs() < 1e-9);
    }

    #[test]
    fn whole_word_beats_substring() {
        let corpus = "# A\n\nthe input value\n\n# B\n\nthe inputs value\n";
        let h = hits(corpus, "input", 2);
        assert_eq!(h[0].section_idx, 0);
        assert!(h[0].score > h[1].score);
    }

    #[test]
    fn top_k_limits_results_and_ties_break_by_document_order() {
        let corpus = "# A\n\nzzz match here\n\n# B\n\nzzz match here\n\n# C\n\nzzz match here\n\n# D\n\nnothing\n";
        let h = hits(corpus, "zzz match", 2);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].section_idx, 0);
        assert_eq!(h[1].section_idx, 1);
    }

    #[test]
    fn bare_heading_orphan_word_is_findable() {
        // "LonelyWord" exists only on a bare heading without any subsection:
        // it must still return a hit (regression: previously lost entirely).
        let corpus = "# Page\n\n## LonelyWord\n\n# Other\n\nunrelated text\n";
        let h = hits(corpus, "lonelyword", 3);
        assert_eq!(h.len(), 1);
        let sections = parse_sections(corpus);
        assert_eq!(sections[h[0].section_idx].heading, "LonelyWord");
    }

    #[test]
    fn bare_heading_does_not_steal_top_k() {
        let corpus = "# Boolean Modifier\n\nthe boolean modifier operates on meshes\n\n## Options\n\n### Solver\n\nsolver choice\n";
        let sections = parse_sections(corpus);
        let opts = sections
            .iter()
            .position(|s| s.heading == "Options")
            .expect("Options section indexed");
        let h = hits(corpus, "boolean modifier", 3);
        // The bare `## Options` (breadcrumb-only match) ranks below both
        // content sections.
        assert_ne!(h[0].section_idx, opts);
        assert_ne!(h[1].section_idx, opts);
        // ...but a query for its own title still finds it.
        assert!(
            hits(corpus, "options", 3)
                .iter()
                .any(|hit| hit.section_idx == opts)
        );
    }

    #[test]
    fn title_prefix_bonus_promotes_page_over_citing_h3() {
        // Mirrors the StandardMaterial3D case: an H1 page whose heading IS
        // the query, against an H3 method of another class citing the term
        // 20 times in body and heading. Without the prefix bonus (or if it
        // leaked to level 3), the H3 would win.
        let body = format!("{}Widget\n", "see Widget usage ".repeat(6));
        let corpus = format!(
            "# Widget\n\nsmall page about one Widget\n\n# Other\n\n### Widget get(x)\n\n{body}"
        );
        let h = hits(&corpus, "Widget", 2);
        let sections = parse_sections(&corpus);
        assert_eq!(sections[h[0].section_idx].heading, "Widget");
        assert_eq!(sections[h[1].section_idx].heading, "Widget get(x)");
    }

    #[test]
    fn title_prefix_bonus_matches_camel_case_identifier() {
        // Multi-word query against a single CamelCase page title.
        let corpus = "# ResourcePreloader\n\nshort description\n\n# Other\n\nthe resource preloader example prints a list, resource preloader again and again\n";
        let h = hits(corpus, "resource preloader", 2);
        let sections = parse_sections(corpus);
        assert_eq!(sections[h[0].section_idx].heading, "ResourcePreloader");
    }

    #[test]
    fn diacritic_folding_is_opt_in() {
        let corpus = "# Résumé\n\nLe résumé du document.\n\n# Other\n\nnothing here\n";
        assert!(hits(corpus, "resume", 2).is_empty());
        let params = RankingParams {
            fold_diacritics: true,
            ..RankingParams::default()
        };
        let sections = parse_sections(corpus);
        let folded = rank_sections(corpus, &sections, "resume", 2, &params).unwrap();
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].section_idx, 0);
    }
}
