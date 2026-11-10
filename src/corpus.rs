//! Corpus loading: a single Markdown file, or a directory of Markdown pages.
//!
//! A directory is walked recursively (hidden entries skipped) and every `.md`
//! file becomes one page of a virtual corpus, in deterministic order (relative
//! path, `/`-separated). Each page is sliced on its own, so a page never
//! inherits the headings of the previous one, and its YAML front matter is
//! stripped from the searchable text; its `url:` field, when present, is kept
//! so results can cite the canonical page.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::SearchError;
use crate::parse::{Section, parse_sections};

/// One page of a directory corpus.
#[derive(Debug, Clone)]
pub struct Source {
    /// Byte range of the page in the concatenated corpus.
    pub range: Range<usize>,
    /// Path relative to the corpus directory, `/`-separated.
    pub path: String,
    /// `url:` field of the page front matter, if any.
    pub url: Option<String>,
}

/// A loaded corpus: concatenated text, its sections, and the pages they come
/// from (empty for a single-file corpus).
pub struct Corpus {
    /// Full searchable text (sections and sources index into it).
    pub text: String,
    /// Sections in corpus order.
    pub sections: Vec<Section>,
    /// Pages, in corpus order; empty for a single-file corpus.
    pub sources: Vec<Source>,
}

/// Load `path`: a Markdown file as-is, or every `.md` file under a directory.
///
/// # Errors
///
/// [`SearchError::Io`] when a file or directory cannot be read,
/// [`SearchError::NoMarkdown`] when a directory holds no `.md` file.
pub fn load(path: &Path) -> Result<Corpus, SearchError> {
    if !path.is_dir() {
        let text = fs::read_to_string(path)?;
        let sections = parse_sections(&text);
        return Ok(Corpus {
            text,
            sections,
            sources: Vec::new(),
        });
    }
    let mut files = Vec::new();
    collect_markdown(path, &mut files)?;
    let mut pages: Vec<(String, PathBuf)> =
        files.into_iter().map(|f| (relative(path, &f), f)).collect();
    pages.sort();
    if pages.is_empty() {
        return Err(SearchError::NoMarkdown(path.display().to_string()));
    }

    let mut text = String::new();
    let mut sections = Vec::new();
    let mut sources = Vec::with_capacity(pages.len());
    for (rel, file) in pages {
        let raw = fs::read_to_string(&file)?;
        let (fm, body) = split_front_matter(&raw);
        let start = text.len();
        // Page name: front matter title plus relative path without `.md`
        // (`docs/git-stash`, `global-section/performance-tuning/maxconn`): a
        // query naming the page, the command or its section of the docs finds
        // it. The path also makes the name unique per page.
        let path = rel.strip_suffix(".md").unwrap_or(&rel);
        let page = match &fm.title {
            Some(title) => format!("{title} {path}"),
            None => path.to_string(),
        };
        let page_stem = bare_stem(&rel);
        for mut section in parse_sections(body) {
            section.range = section.range.start + start..section.range.end + start;
            section.page.clone_from(&page);
            section.page_stem.clone_from(&page_stem);
            sections.push(section);
        }
        text.push_str(body);
        if !text.ends_with('\n') {
            text.push('\n');
        }
        sources.push(Source {
            range: start..text.len(),
            path: rel,
            url: fm.url,
        });
    }
    Ok(Corpus {
        text,
        sections,
        sources,
    })
}

/// The page holding byte `offset`, if the corpus is a directory.
#[must_use]
pub fn source_at(sources: &[Source], offset: usize) -> Option<&Source> {
    let idx = sources.partition_point(|s| s.range.end <= offset);
    sources.get(idx).filter(|s| s.range.contains(&offset))
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), SearchError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_markdown(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Lowercase file stem without `.md` nor a man-page section suffix:
/// `openssh-client/ssh-keygen.1.en.md` -> `ssh-keygen`, `pm.max_children.md`
/// -> `pm.max_children`.
pub(crate) fn bare_stem(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let mut stem = name.strip_suffix(".md").unwrap_or(name).to_lowercase();
    if let Some(s) = stem.strip_suffix(".en") {
        stem = s.to_string();
    }
    if let Some((head, tail)) = stem.rsplit_once('.') {
        if !tail.is_empty() && tail.len() <= 2 && tail.chars().all(|c| c.is_ascii_digit()) {
            stem = head.to_string();
        }
    }
    stem
}

/// Fields kept from a page's YAML front matter.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct FrontMatter {
    pub(crate) url: Option<String>,
    pub(crate) title: Option<String>,
}

/// Split a leading YAML front matter (`---` ... `---`) off `raw`: returns its
/// `url:` and `title:` values (unquoted) and the remaining Markdown. Text
/// without front matter, or with an unterminated one, is returned whole.
pub(crate) fn split_front_matter(raw: &str) -> (FrontMatter, &str) {
    let content = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = content.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return (FrontMatter::default(), content);
    }
    let mut offset = content.find('\n').map_or(content.len(), |i| i + 1);
    let mut fm = FrontMatter::default();
    for line in lines {
        offset += line.len();
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return (fm, &content[offset..]);
        }
        for (key, slot) in [("url:", &mut fm.url), ("title:", &mut fm.title)] {
            if let Some(value) = trimmed.strip_prefix(key) {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                if !value.is_empty() {
                    *slot = Some(value.to_string());
                }
            }
        }
    }
    (FrontMatter::default(), content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_stem_drops_extension_and_man_section() {
        assert_eq!(bare_stem("openssh-client/ssh-keygen.1.en.md"), "ssh-keygen");
        assert_eq!(bare_stem("man/jail.conf.5.md"), "jail.conf");
        assert_eq!(
            bare_stem("install.fpm.configuration/pm.max_children.md"),
            "pm.max_children"
        );
        assert_eq!(bare_stem("mod/core/AllowOverride.md"), "allowoverride");
        assert_eq!(bare_stem("journald.conf.md"), "journald.conf");
    }

    #[test]
    fn front_matter_is_split_and_url_kept() {
        let raw =
            "---\ntitle: \"Page\"\nurl: https://example.org/page.html\n---\n\n# Page\n\nbody\n";
        let (fm, body) = split_front_matter(raw);
        assert_eq!(fm.url.as_deref(), Some("https://example.org/page.html"));
        assert_eq!(fm.title.as_deref(), Some("Page"));
        assert_eq!(body, "\n# Page\n\nbody\n");
    }

    #[test]
    fn quoted_url_and_crlf_and_bom_are_handled() {
        let raw = "\u{feff}---\r\nurl: 'https://e.org/x'\r\n---\r\n# X\r\n";
        let (fm, body) = split_front_matter(raw);
        assert_eq!(fm.url.as_deref(), Some("https://e.org/x"));
        assert_eq!(body, "# X\r\n");
    }

    #[test]
    fn text_without_or_with_unterminated_front_matter_is_untouched() {
        let none = || FrontMatter::default();
        assert_eq!(split_front_matter("# Title\n"), (none(), "# Title\n"));
        let unterminated = "---\nurl: https://e.org\n# Title\n";
        assert_eq!(split_front_matter(unterminated), (none(), unterminated));
        // A thematic break later in the page is not front matter.
        assert_eq!(split_front_matter("text\n---\n"), (none(), "text\n---\n"));
    }

    #[test]
    fn source_lookup_by_offset() {
        let sources = vec![
            Source {
                range: 0..10,
                path: "a.md".into(),
                url: None,
            },
            Source {
                range: 10..25,
                path: "b.md".into(),
                url: Some("u".into()),
            },
        ];
        assert_eq!(source_at(&sources, 0).unwrap().path, "a.md");
        assert_eq!(source_at(&sources, 9).unwrap().path, "a.md");
        assert_eq!(source_at(&sources, 10).unwrap().path, "b.md");
        assert!(source_at(&sources, 25).is_none());
        assert!(source_at(&[], 3).is_none());
    }
}
