//! Section slicing: turns raw corpus Markdown into [`Section`]s.
//!
//! The scanner is line-based and CommonMark-flavoured where it matters for
//! search:
//!
//! - fenced code blocks (backtick or tilde fences) never emit headings,
//!   whatever their content (a `# comment` inside a Python block stays body
//!   text);
//! - a closing fence must use the same character and be at least as long as
//!   the opening one;
//! - headings may be indented by 0-3 spaces; deeper indentation is code and
//!   never a heading;
//! - Setext underlines (`Title` followed by `====` or `----`) start a section,
//!   but a `---` that follows a blank line stays a thematic break;
//! - `####` and deeper do not split sections;
//! - the text before the first heading is indexed as a preamble section
//!   (level 0) instead of being dropped;
//! - sections whose body is empty (a bare heading like `## Options`) stay in
//!   the index with `has_body = false` — their title text remains findable,
//!   and ranking penalizes them so they cannot steal a top-k slot from
//!   sections with real content.

use std::ops::Range;

/// One searchable slice of the corpus.
#[derive(Debug, Clone)]
pub struct Section {
    /// Byte range of the section body in the corpus (heading line excluded).
    pub range: Range<usize>,
    /// Heading level: 0 for the preamble (text before the first heading),
    /// 1-3 otherwise.
    pub level: u8,
    /// Own heading text without the leading `#`s (empty for the preamble).
    pub heading: String,
    /// Titles of the enclosing headings, outermost first (page H1, then H2).
    pub breadcrumb: Vec<String>,
    /// False for a bare heading (no body until the next heading). Such
    /// sections stay indexed so their title text is findable, but ranking
    /// penalizes them (see `RankingParams::empty_body_factor`).
    pub has_body: bool,
}

/// An open fenced code block.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fence {
    pub(crate) ch: u8,
    pub(crate) len: usize,
}

/// If `line` opens a fenced code block, its fence character and length.
pub(crate) fn fence_opener(line: &str) -> Option<Fence> {
    let rest = strip_up_to_3_spaces(line)?;
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|&b| b == ch).count();
    if len < 3 {
        return None;
    }
    let info = &rest[len..];
    // CommonMark: the info string of a backtick fence must not contain backticks.
    if ch == b'`' && info.contains('`') {
        return None;
    }
    Some(Fence { ch, len })
}

/// True if `line` closes a fence opened with `fence`.
pub(crate) fn fence_closer(line: &str, fence: Fence) -> bool {
    let Some(rest) = strip_up_to_3_spaces(line) else {
        return false;
    };
    let run = rest.bytes().take_while(|&b| b == fence.ch).count();
    run >= fence.len && rest[run..].trim().is_empty()
}

/// The line stripped of up to 3 leading spaces, or None when indented 4+
/// spaces (an indented code block line can never be a heading or a fence).
fn strip_up_to_3_spaces(line: &str) -> Option<&str> {
    let stripped = line.trim_start_matches(' ');
    (line.len() - stripped.len() < 4).then_some(stripped)
}

/// ATX heading `(level, text)` of `line`, levels 1-3 only.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let rest = strip_up_to_3_spaces(line)?;
    let mut level = 0u8;
    for &b in rest.as_bytes() {
        if b == b'#' {
            level += 1;
        } else {
            break;
        }
    }
    if level == 0 || level > 3 {
        return None;
    }
    let after = &rest[usize::from(level)..];
    let text = after.trim();
    if !text.is_empty() {
        // `#foo` is not a heading: a space or tab must follow the hashes.
        let b = rest.as_bytes()[usize::from(level)];
        if b != b' ' && b != b'\t' {
            return None;
        }
    }
    Some((level, text))
}

/// Setext underline level (1 for `=`, 2 for `-`) if `line` is a bare
/// underline run; combined with a pending paragraph, it closes a heading.
fn setext_underline(line: &str) -> Option<u8> {
    let rest = strip_up_to_3_spaces(line)?;
    let run = rest.trim_end();
    let ch = *run.as_bytes().first()?;
    if ch != b'=' && ch != b'-' {
        return None;
    }
    run.bytes()
        .all(|b| b == ch)
        .then_some(if ch == b'=' { 1 } else { 2 })
}

/// Split `content` into sections, in document order.
#[must_use]
pub fn parse_sections(content: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    // Open headings, current section on top: (level, title).
    let mut stack: Vec<(u8, String)> = Vec::new();
    let mut fence: Option<Fence> = None;

    let mut body_start = 0usize; // where the current section body starts
    let mut level = 0u8; // current section level (0 = preamble)
    let mut heading = String::new();
    let mut breadcrumb: Vec<String> = Vec::new();

    // Pending paragraph, kept to detect Setext underlines.
    let mut para_start: Option<usize> = None;
    let mut para_text = String::new();

    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        if let Some(open) = fence {
            if fence_closer(line, open) {
                fence = None;
            }
            continue; // inside code: never a heading, never a paragraph line
        }
        if let Some(open) = fence_opener(line) {
            fence = Some(open);
            reset_paragraph(&mut para_start, &mut para_text);
            continue; // the fence line itself is body
        }

        if let Some((lvl, text)) = atx_heading(line) {
            flush(
                &mut sections,
                content,
                body_start..line_start,
                level,
                &heading,
                &breadcrumb,
            );
            pop_shallower(&mut stack, lvl);
            stack.push((lvl, text.to_string()));
            level = lvl;
            heading = text.to_string();
            breadcrumb = ancestor_titles(&stack);
            body_start = offset;
            reset_paragraph(&mut para_start, &mut para_text);
            continue;
        }

        if let Some(lvl) = setext_underline(line) {
            if let Some(ps) = para_start {
                // The pending paragraph becomes the heading of a new section;
                // the current section body ends where that paragraph started.
                flush(
                    &mut sections,
                    content,
                    body_start..ps,
                    level,
                    &heading,
                    &breadcrumb,
                );
                pop_shallower(&mut stack, lvl);
                let text = std::mem::take(&mut para_text);
                para_start = None;
                stack.push((lvl, text.clone()));
                level = lvl;
                heading = text;
                breadcrumb = ancestor_titles(&stack);
                body_start = offset;
                continue;
            }
            // Bare thematic break: ordinary body line.
            reset_paragraph(&mut para_start, &mut para_text);
            continue;
        }

        if line.trim().is_empty() {
            reset_paragraph(&mut para_start, &mut para_text);
            continue;
        }
        // Ordinary body line: extends or starts the pending paragraph. Only
        // lines indented by less than 4 spaces may START a paragraph (deeper
        // indentation is an indented code block).
        let indent = line.len() - line.trim_start_matches(' ').len();
        let text = line.trim();
        if para_start.is_none() && indent < 4 {
            para_start = Some(line_start);
            para_text = text.to_string();
        } else if para_start.is_some() {
            para_text.push(' ');
            para_text.push_str(text);
        }
    }

    flush(
        &mut sections,
        content,
        body_start..content.len(),
        level,
        &heading,
        &breadcrumb,
    );
    sections
}

fn reset_paragraph(para_start: &mut Option<usize>, para_text: &mut String) {
    *para_start = None;
    para_text.clear();
}

fn pop_shallower(stack: &mut Vec<(u8, String)>, level: u8) {
    while stack.last().is_some_and(|&(l, _)| l >= level) {
        stack.pop();
    }
}

fn ancestor_titles(stack: &[(u8, String)]) -> Vec<String> {
    stack[..stack.len() - 1]
        .iter()
        .map(|(_, title)| title.clone())
        .collect()
}

/// Push a section. Bare headings (empty body) are kept in the index so a
/// word that exists only on such a heading remains findable; ranking
/// penalizes them via `has_body`.
fn flush(
    sections: &mut Vec<Section>,
    content: &str,
    range: Range<usize>,
    level: u8,
    heading: &str,
    breadcrumb: &[String],
) {
    let has_body = !content[range.clone()].trim().is_empty();
    // An empty preamble (file starting with a heading) holds nothing
    // indexable; bare headings (heading without body) are kept.
    if heading.is_empty() && !has_body {
        return;
    }
    sections.push(Section {
        range,
        level,
        heading: heading.to_string(),
        breadcrumb: breadcrumb.to_vec(),
        has_body,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headings(content: &str) -> Vec<(u8, String, Vec<String>)> {
        parse_sections(content)
            .into_iter()
            .map(|s| (s.level, s.heading, s.breadcrumb))
            .collect()
    }

    fn bodies(content: &str) -> Vec<String> {
        parse_sections(content)
            .into_iter()
            .map(|s| content[s.range.clone()].to_string())
            .collect()
    }

    #[test]
    fn heading_inside_code_fence_is_body() {
        let content = "# Page\n\n```python\n# not a heading\nx = 1\n```\n\n## Real\n\nbody\n";
        assert_eq!(
            headings(content),
            vec![
                (1u8, "Page".to_string(), vec![]),
                (2u8, "Real".to_string(), vec!["Page".to_string()]),
            ]
        );
        assert!(bodies(content)[0].contains("# not a heading"));
    }

    #[test]
    fn tilde_fence_is_respected() {
        let content = "# Page\n\n~~~\n# not a heading either\n~~~\n\n## Real\n\nbody\n";
        assert_eq!(headings(content).len(), 2);
        assert!(bodies(content)[0].contains("# not a heading either"));
    }

    #[test]
    fn closing_fence_must_be_at_least_as_long() {
        let content = "# T\n\n````\n``` inner\nstill code\n````\n\n## After\n\nb\n";
        assert_eq!(headings(content).len(), 2);
        assert!(bodies(content)[0].contains("still code"));
    }

    #[test]
    fn preamble_is_indexed() {
        let content = "Intro text.\n\n# Page\n\nbody\n";
        let secs = parse_sections(content);
        assert_eq!(secs[0].level, 0);
        assert!(secs[0].heading.is_empty());
        assert_eq!(&content[secs[0].range.clone()], "Intro text.\n\n");
        assert_eq!(secs[1].heading, "Page");
    }

    #[test]
    fn setext_headings_split() {
        let content = "Title line\n===\n\nbody one\n\nSub\n---\n\nbody two\n";
        assert_eq!(
            headings(content),
            vec![
                (1u8, "Title line".to_string(), vec![]),
                (2u8, "Sub".to_string(), vec!["Title line".to_string()]),
            ]
        );
        let bs = bodies(content);
        assert_eq!(bs[0], "\nbody one\n\n");
        assert_eq!(bs[1], "\nbody two\n");
    }

    #[test]
    fn thematic_break_after_blank_line_is_not_a_heading() {
        let content = "Para\n\n---\n\n# Page\n\nb\n";
        assert_eq!(
            headings(content),
            vec![
                (0u8, String::new(), vec![]),
                (1u8, "Page".to_string(), vec![])
            ]
        );
        assert!(bodies(content)[0].contains("---"));
    }

    #[test]
    fn heading_indent_and_hash_rules() {
        let content = "    # indented code\n\n# yes\n\n##also not\n\n  ## also\n\nbody of also\n\n   ### three\n\n#### four stays body\n";
        assert_eq!(
            headings(content)
                .into_iter()
                .map(|(l, t, _)| (l, t))
                .collect::<Vec<_>>(),
            vec![
                (0u8, String::new()),
                (1u8, "yes".to_string()),
                (2u8, "also".to_string()),
                (3u8, "three".to_string()),
            ]
        );
        let secs = parse_sections(content);
        let find = |name: &str| secs.iter().find(|s| s.heading == name).unwrap();
        assert_eq!(find("also").breadcrumb, ["yes"]);
        assert_eq!(find("three").breadcrumb, ["yes", "also"]);
        assert!(bodies(content)[0].contains("# indented code"));
    }

    #[test]
    fn h4_does_not_split() {
        let content = "# P\n\n#### Deep\n\nstill same section\n";
        assert_eq!(headings(content).len(), 1);
        assert!(bodies(content)[0].contains("#### Deep"));
    }

    #[test]
    fn h3_inherits_h2_breadcrumb_and_bare_headings_are_penalized() {
        let content = "# Page\n\n## Opts\n\n### Solver\n\ns body\n\n### Other\n\no body\n\n## Tail\n\nt body\n";
        let secs = parse_sections(content);
        let find = |name: &str| secs.iter().find(|s| s.heading == name).unwrap();
        assert_eq!(find("Solver").breadcrumb, ["Page", "Opts"]);
        assert_eq!(find("Other").breadcrumb, ["Page", "Opts"]);
        assert_eq!(find("Tail").breadcrumb, ["Page"]);
        // The bare `## Options` heading stays indexed but flagged bodyless.
        assert!(!find("Opts").has_body);
        assert!(find("Solver").has_body);
    }

    #[test]
    fn crlf_and_no_trailing_newline() {
        let content = "# P\r\n\r\nbody line\r\n## Tail";
        let secs = parse_sections(content);
        // The trailing bare heading stays indexed (flagged bodyless).
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].heading, "P");
        assert!(secs[0].has_body);
        assert_eq!(secs[1].heading, "Tail");
        assert!(!secs[1].has_body);
        assert!(bodies(content)[0].contains("body line"));
    }

    #[test]
    fn file_without_headings_is_one_preamble_section() {
        let secs = parse_sections("just\nplain text\n");
        assert_eq!(secs.len(), 1);
        assert_eq!(secs[0].level, 0);
    }
}
