//! Keyword scoring: IDF weighting, heading/phrase/whole-word bonuses, coverage.

use std::collections::{HashMap, HashSet};

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
    /// Page identity (directory corpora): bonus when a keyword is a whole word
    /// of the page name (front matter title and relative path, see
    /// `Section::page`). Added after length normalization.
    pub page_name_bonus: f64,
    /// Page identity: bonus when the page file stem equals the whole query
    /// (`allowoverride.md` for `AllowOverride`, `timeout-connect.md` for
    /// `timeout connect`). Added after length normalization.
    pub stem_query_bonus: f64,
    /// Page identity: bonus when the page file stem equals one keyword
    /// (`journalctl.md` for `journalctl since until`). Added after length
    /// normalization.
    pub stem_keyword_bonus: f64,
    /// Bonus when a keyword starts a body line (after list/emphasis/code
    /// markup): the shape of a definition in reference pages (`**MaxRetry**`,
    /// `` `ssl_protocols` ``, `- PermitRootLogin`). Added after length
    /// normalization, so a definition buried in a long man page still counts.
    pub definition_bonus: f64,
    /// Directory corpora: at most this many sections of the same page in the
    /// top-k, so one long page cannot fill every slot.
    pub max_sections_per_page: usize,
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
            page_name_bonus: 10.0,
            stem_query_bonus: 30.0,
            stem_keyword_bonus: 10.0,
            definition_bonus: 5.0,
            max_sections_per_page: 2,
            fold_diacritics: false,
        }
    }
}

/// True when `kw` starts a line of `body` once leading markup is skipped.
fn starts_a_line(body: &str, kw: &str) -> bool {
    body.lines().any(|line| {
        line.trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '*' | '`' | '-' | '+' | '|' | '>' | '_' | '[' | '"')
        })
        .starts_with(kw)
    })
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

/// Strip sentence punctuation wrapped around a query keyword: quotes and
/// brackets on either side, trailing `, ; : ! ? .` and empty call parentheses.
/// `"boolean`, `anisotropy?` and `(merge_threshold)` become plain words,
/// which a substring scan would otherwise never find. Inner punctuation and
/// symbols that are part of a name are kept: `Input.is_action_just_pressed`,
/// `C#`, `C++`, `--force`, `.NET`, `Vec<T>`.
pub(crate) fn trim_query_punctuation(word: &str) -> &str {
    const LEADING: &[char] = &['(', '[', '{', '"', '\'', '«', '“', '‘', '¿', '¡', '`'];
    const TRAILING: &[char] = &[
        ')', ']', '}', '(', '[', '{', '"', '\'', '»', '”', '’', '`', ',', ';', ':', '!', '?', '.',
    ];
    word.trim_start_matches(LEADING).trim_end_matches(TRAILING)
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
        .map(trim_query_punctuation)
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
    let pages: Vec<String> = sections.iter().map(|s| normalize(&s.page)).collect();

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
        let page = &pages[si];
        let body = &bodies[si];
        let mut score = 0.0f64;
        // Identity bonuses (page name, file stem, definition line; directory
        // corpora only) say *which* section the query is about, whatever its
        // length: they are added after length normalization, so a definition
        // buried in a long man page still counts.
        let mut identity = 0.0f64;
        let mut matched = 0usize;
        for ki in 0..k {
            let kw = keywords[ki].as_str();
            let weight = weights[ki];
            let in_heading = head.contains(kw);
            let in_crumb = crumb.contains(kw);
            let in_page = !page.is_empty() && count_matches(page, kw).1 > 0;
            if occ[si * k + ki] > 0 || in_heading || in_crumb || in_page {
                matched += 1;
            }
            if in_page {
                identity += params.page_name_bonus * weight;
            }
            // Directory corpora only: measured there (scripts/relevance/),
            // single-file corpora keep their previous ranking.
            if !page.is_empty() && occ[si * k + ki] > 0 && starts_a_line(body, kw) {
                identity += params.definition_bonus * weight;
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
        if score + identity <= 0.0 {
            continue;
        }

        // All query terms somewhere in the heading chain (page name included):
        // strong context signal.
        if keywords.iter().all(|kw| {
            head.contains(kw.as_str()) || crumb.contains(kw.as_str()) || page.contains(kw.as_str())
        }) {
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
        // Page named exactly like the query (`allowoverride.md`,
        // `timeout-connect.md`), or else like one of its keywords
        // (`journalctl.md` for "journalctl since until").
        let stem = sections[si].page_stem.as_str();
        if !stem.is_empty() {
            if ["", "-", "_", ".", " "]
                .iter()
                .any(|sep| keywords.join(sep) == stem)
            {
                identity += params.stem_query_bonus * avg_weight;
            } else {
                for ki in 0..k {
                    if keywords[ki] == stem {
                        identity += params.stem_keyword_bonus * weights[ki];
                    }
                }
            }
        }
        // Coverage: a section must speak to the whole query to keep its full
        // score, so one repeated term cannot outrank a section covering them all.
        let coverage = matched as f64 / k as f64;
        if matched < k {
            score *= coverage;
        }
        // Length normalization: giant sections cannot win by being giant.
        let body_bytes = sections[si].range.len();
        score /= 1.0 + (body_bytes as f64 / params.len_norm_bytes).sqrt();
        score += identity * coverage;
        // Bare headings stay findable but cannot outrank content sections.
        if !sections[si].has_body {
            score *= params.empty_body_factor;
        }
        scored.push((score, si));
    }

    if scored.is_empty() {
        return Ok(Vec::new());
    }
    // Best first, keeping only the first of identical sections (same heading
    // and same body once normalized): mirrored docs often ship verbatim copies
    // (man-page aliases such as `rlogin.1` = `ssh.1`, `foo.conf.d` = `foo.conf`)
    // that would otherwise fill every top-k slot with the same text.
    scored.sort_by(hit_order);
    // Directory corpora also cap the sections taken from one page (the page
    // name holds its unique relative path).
    let mut seen: HashSet<(&str, &str)> = HashSet::with_capacity(top_k);
    let mut per_page: HashMap<&str, usize> = HashMap::new();
    Ok(scored
        .iter()
        .filter(|&&(_, si)| seen.insert((headings[si].as_str(), bodies[si].trim())))
        .filter(|&&(_, si)| {
            let page = sections[si].page.as_str();
            if page.is_empty() {
                return true;
            }
            let taken = per_page.entry(page).or_insert(0);
            *taken += 1;
            *taken <= params.max_sections_per_page
        })
        .take(top_k)
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

    /// A directory-like corpus: one `(relative path, markdown)` per page,
    /// sections tagged with their page name and stem as `corpus::load` does.
    fn dir_hits(pages: &[(&str, &str)], query: &str, top_k: usize) -> Vec<String> {
        let mut text = String::new();
        let mut sections = Vec::new();
        for (rel, md) in pages {
            let start = text.len();
            for mut s in parse_sections(md) {
                s.range = s.range.start + start..s.range.end + start;
                s.page = rel.trim_end_matches(".md").to_string();
                s.page_stem = crate::corpus::bare_stem(rel);
                sections.push(s);
            }
            text.push_str(md);
        }
        rank_sections(&text, &sections, query, top_k, &RankingParams::default())
            .unwrap()
            .iter()
            .map(|h| sections[h.section_idx].page.clone())
            .collect()
    }

    #[test]
    fn page_name_promotes_the_page_about_the_query() {
        // Same words in both bodies: only the page name tells them apart.
        let pages = [
            ("book/tools.md", "# Tools\n\nkeep your changes in a stash\n"),
            (
                "docs/git-stash.md",
                "# Synopsis\n\nkeep your changes in a stash\n",
            ),
        ];
        assert_eq!(dir_hits(&pages, "stash changes", 2)[0], "docs/git-stash");
    }

    #[test]
    fn stem_equal_to_query_wins_over_a_longer_name() {
        let body = "# Directive\n\nAllowOverride controls .htaccess\n";
        let pages = [
            ("core/allowoverridelist.md", body),
            ("core/allowoverride.md", body),
        ];
        assert_eq!(
            dir_hits(&pages, "AllowOverride", 2)[0],
            "core/allowoverride"
        );
    }

    #[test]
    fn definition_line_counts_even_in_a_long_section() {
        let filler = "unrelated option text that goes on and on\n".repeat(400);
        let reference = format!(
            "# OPTIONS\n\n{filler}**PermitRootLogin**\n  Specifies whether root can log in.\n{filler}"
        );
        let pages = [
            (
                "ssh.md",
                "# DESCRIPTION\n\nsee sshd for PermitRootLogin details\n".to_string(),
            ),
            ("sshd_config.md", reference),
        ];
        let pages: Vec<(&str, &str)> = pages.iter().map(|(p, m)| (*p, m.as_str())).collect();
        assert_eq!(dir_hits(&pages, "PermitRootLogin", 2)[0], "sshd_config");
    }

    #[test]
    fn one_page_cannot_fill_the_top_k() {
        let long = "# A\n\nkey term\n\n# B\n\nkey term\n\n# C\n\nkey term\n";
        let pages = [("long.md", long), ("other.md", "# D\n\nkey other\n")];
        let got = dir_hits(&pages, "key", 3);
        assert_eq!(got.iter().filter(|p| *p == "long").count(), 2);
        assert!(got.contains(&"other".to_string()));
    }

    #[test]
    fn single_file_corpus_gets_no_identity_bonus() {
        // Same shape as the long-section test, but as one file: no page, so
        // the definition line earns nothing and the short section wins.
        let filler = "unrelated option text that goes on and on\n".repeat(400);
        let corpus = format!(
            "# DESCRIPTION\n\nsee sshd for PermitRootLogin details\n\n# OPTIONS\n\n{filler}**PermitRootLogin**\n{filler}"
        );
        let sections = parse_sections(&corpus);
        let h = hits(&corpus, "PermitRootLogin", 1);
        assert_eq!(sections[h[0].section_idx].heading, "DESCRIPTION");
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
    fn identical_sections_take_one_top_k_slot() {
        // Three verbatim copies of the same section (man-page aliases) and one
        // distinct section: the copies must not fill the top-k.
        let copy = "## OPTIONS\n\nForwardAgent enables forwarding of the agent\n\n";
        let corpus = format!(
            "# ssh\n\n{copy}# rlogin\n\n{copy}# slogin\n\n{copy}# ssh_config\n\nForwardAgent yes or no, forward agent\n"
        );
        let sections = parse_sections(&corpus);
        let h = hits(&corpus, "forwardagent", 3);
        assert_eq!(h.len(), 2);
        // Page of a hit: its H1 ancestor, or its own heading for an H1 section.
        let pages: Vec<&str> = h
            .iter()
            .map(|hit| {
                let s = &sections[hit.section_idx];
                s.breadcrumb.first().unwrap_or(&s.heading).as_str()
            })
            .collect();
        // The first copy (document order) stands for all of them.
        assert!(pages.contains(&"ssh"));
        assert!(pages.contains(&"ssh_config"));
        // Same heading but different body: both kept.
        let corpus = "# A\n\n## Options\n\nmerge distance\n\n# B\n\n## Options\n\nmerge angle\n";
        assert_eq!(hits(corpus, "merge", 3).len(), 2);
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
    fn query_punctuation_is_trimmed() {
        assert_eq!(trim_query_punctuation("anisotropy?"), "anisotropy");
        assert_eq!(
            trim_query_punctuation("(merge_threshold)"),
            "merge_threshold"
        );
        assert_eq!(trim_query_punctuation("move_and_slide()"), "move_and_slide");
        assert_eq!(trim_query_punctuation("\"boolean"), "boolean");
        assert_eq!(trim_query_punctuation("modifier\","), "modifier");
        assert_eq!(trim_query_punctuation("«résumé»"), "résumé");
        // Name-forming symbols survive.
        assert_eq!(
            trim_query_punctuation("Input.is_action_just_pressed"),
            "Input.is_action_just_pressed"
        );
        assert_eq!(trim_query_punctuation("C#"), "C#");
        assert_eq!(trim_query_punctuation("C++"), "C++");
        assert_eq!(trim_query_punctuation("--force"), "--force");
        assert_eq!(trim_query_punctuation(".NET"), ".NET");
        assert_eq!(trim_query_punctuation("Vec<T>"), "Vec<T>");
        // Pure punctuation vanishes (then dropped as an empty keyword).
        assert_eq!(trim_query_punctuation("?!"), "");
    }

    #[test]
    fn punctuated_query_finds_the_same_section() {
        let corpus = "# Mirror\n\n**merge_threshold**(distance: float) merges vertices\n\n# Boolean Modifier\n\nthe boolean modifier operates on meshes\n\n# Other\n\nunrelated text\n";
        let sections = parse_sections(corpus);
        for (plain, punctuated) in [
            ("merge_threshold", "(merge_threshold)?"),
            ("boolean modifier", "\"boolean modifier\""),
        ] {
            let a = hits(corpus, plain, 1);
            let b = hits(corpus, punctuated, 1);
            assert_eq!(b.len(), 1, "{punctuated} must match");
            assert_eq!(
                sections[a[0].section_idx].heading,
                sections[b[0].section_idx].heading
            );
        }
        // A query made only of punctuation holds no keyword.
        assert!(matches!(
            rank_sections(corpus, &sections, "?? !", 1, &RankingParams::default()),
            Err(SearchError::EmptyQuery)
        ));
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
