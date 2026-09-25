![md-doc-search banner](assets/header.jpg)

# md-doc-search

**Offline full-text search over version-pinned Markdown documentation — built to ground AI coding agents on the exact docs version they target.**

![Rust](https://img.shields.io/badge/rust-1.85%2B-orange) ![License](https://img.shields.io/badge/license-MIT-blue) ![Platform](https://img.shields.io/badge/platform-windows%20%7C%20linux%20%7C%20macos-lightgrey) [![CI](https://github.com/laurentvv/md-doc-search/actions/workflows/ci.yml/badge.svg)](https://github.com/laurentvv/md-doc-search/actions/workflows/ci.yml)

A small dependency-light Rust binary that answers *precise* technical queries (`Boolean Modifier`, `Input is_action_just_pressed`, `Volume Scatter`) against a local Markdown snapshot of the documentation you actually target — in ~40 ms, with no network, no server, and no index to maintain.

## The problem it solves

LLM coding agents hallucinate APIs, and web documentation makes it worse: search results blend every version ever published. When your agent writes code against **Blender 5.2** or **Godot 4.7**, it needs *that* version's docs — not a blend of 2022 Stack Overflow answers.

`md-doc-search` turns a documentation snapshot into a deterministic, citation-friendly lookup tool your agent can call from a shell:

```text
$ md-doc-search godot_docs.md "Input is_action_just_pressed" 500
Résultats de recherche pour 'Input is_action_just_pressed' :

--- DÉBUT DE SECTION (Score: 156) ---
### is_action_just_pressed(action: , exact_match: = false) const
> Input
Returns `true` when the user has started pressing the action event...
--- FIN DE SECTION ---
```

Real-world corpora we run daily through this exact binary:

| Corpus | Size | Content | Query time |
|---|---|---|---|
| Blender 5.2 LTS manual | 10.5 MB | full manual, 3 118 referenced screenshots | ~70 ms |
| Godot 4.7 stable docs | 10.7 MB | 1 076 class pages + 392 tutorials | ~40 ms |
| Roblox Creator docs | 22 MB | 2 280 guide pages via official llms.txt | ~95 ms |
| Roblox Engine API reference | 11.6 MB | 1 194 class pages | ~55 ms |

## Install

```bash
git clone https://github.com/laurentvv/md-doc-search
cd md-doc-search
cargo build --release
# binary: target/release/md-doc-search(.exe)
# or system-wide:
cargo install --path .
```

Requires Rust 1.85+ (edition 2024). Single dependency: `clap` (argument parsing) — no regex engine, no server, no index.

## Usage

```bash
md-doc-search <markdown_file> "<keywords>" [max_tokens] [top_k]
md-doc-search <markdown_file> "<keywords>" --max-tokens 800 --top-k 3
```

| Argument | Default | Description |
|---|---|---|
| `markdown_file` | — | path to any Markdown corpus |
| `keywords` | — | query, as a single argument (spaces allowed, **English keywords** for English docs) |
| `max_tokens` / `--max-tokens` | `8000` | output budget (1 token ≈ 4 chars); sections are truncated to fit |
| `top_k` / `--top-k` | `3` | number of sections returned |

The legacy positional form is kept for existing `SKILL.md` files; flags are recommended for new integrations. Passing the same parameter both ways is an error, and so are non-numeric or zero values — nothing is silently replaced by a default. `--help` and `--version` are available, and `--fold-diacritics` matches accented text through its ASCII base (`é` → `e`) for non-English corpora.

Exit codes follow `grep` conventions, so agents can react programmatically:

| Code | Meaning |
|---|---|
| `0` | at least one section returned |
| `1` | no section matches the query |
| `2` | error (unreadable corpus, invalid argument) — details on **stderr** |

Try it immediately on the bundled sample:

```bash
md-doc-search examples/demo.md "boolean modifier"
```

## How ranking works

The corpus is split by a CommonMark-aware line scanner: `#`/`##`/`###` ATX headings **and** Setext underlines (`Title` + `====`/`----`) start a section; fenced code blocks (```` ``` ````/`~~~`) never emit headings, so a `# comment` inside a Python or Bash block stays body text; the text before the first heading is indexed too; `####` and deeper do **not** split sections. Each section remembers its heading ancestry — a `### Solver` section under `## Options` inherits the `# Boolean Modifier` page context.

Sections are then scored:

1. **IDF-weighted keywords** — terms rare across the corpus (`boolean`, `anisotropy`) weigh far more than ubiquitous ones (`modifier`, `options`). This alone kills the classic failure where generic "## Options" boilerplate outranks the page you want.
2. **Heading bonuses** — matches in the section's own heading (+15 × weight) and in its ancestor headings (+10 × weight); ×1.4 bonus when *all* query terms appear in the heading chain.
3. **Title-prefix bonus** — an H1/H2 heading that starts with the first keyword and contains every keyword (including CamelCase identifiers: `ResourcePreloader` for "resource preloader") gets +25 × weight, so class pages outrank method signatures that merely cite the term.
4. **Whole-word bonus** — `input` scores higher when it matches the word `input` than when it only matches inside `inputs`.
5. **Exact-phrase bonus** for the full keyword sequence (normalized: case and repeated spaces don't matter; skipped for single-keyword queries).
6. **Coverage factor** — a section matching only part of the query keeps a proportional share of its score, so one repeated term cannot outrank a section covering them all.
7. **Occurrence cap (20)** and **length normalization** — giant sections can't win by being giant. Bare headings (a `## Options` with no body) stay indexed — a word existing *only* on such a heading is still found — but their score is scaled (×0.2) so they don't steal a top-k slot from content sections.
8. **Token budget** — output is capped at `max_tokens` (≈4 chars/token, counted in *characters*, not bytes). Sections get a greedy share of the budget: what short sections leave unused flows to longer ones. Truncation lands on a line boundary and closes a code fence left open by the cut, with an explicit truncation marker (agents can raise the budget and re-query).

Sort is deterministic (score, then document order).

## Grounding your AI agent

The intended deployment is a tiny agent *skill* per corpus: a `SKILL.md` that hard-codes the corpus path, mandates English keywords, and tells the agent how to read the output. Minimal example:

```markdown
---
name: godot-doc-search
description: Search the local Godot 4.7 docs. Use whenever a Godot feature,
  class, method, signal, node or GDScript question comes up.
---
# Godot Doc Search
"C:/GIT/md-doc-search/target/release/md-doc-search.exe" \
  "C:/GIT/md-doc-search/docs/godot/godot_docs_stable_47_clean.md" \
  "<english keywords>" --max-tokens 800

Exit codes: 0 = results (read them), 1 = no match (retry with more precise
identifiers like "ClassName method_name"), 2 = error (see stderr).
Each result block starts with the section heading, then a `> Page > Section`
breadcrumb line giving its context.
```

Notes: prefer the `_clean.md` corpus when it exists (fences repaired, navigation stripped — see [Post-processing & normalization](#2-post-processing--normalization) for measured impact); regenerate after each crawl with `python scripts/fetch_docs.py <corpus>`.

Practices that make it work well in production:

- **Pin the corpus, pin the version** — treat docs like a lockfile: the snapshot lives on disk, dated, and the skill references that exact file.
- **Always query in the corpus language** (usually English), even from non-English conversations.
- **Prefer `ClassName methodName` queries** — with heading-aware scoring, the exact method section comes back first.
- **Keep resolvable references**: keep screenshots on disk next to the corpus and keep canonical URLs as blockquote lines (`> Source: https://...`) so the agent can cite or fetch the original page.

> [!TIP]
> **Recommended Pipeline**: Use **[crawl4ai-mcp-llm](https://github.com/laurentvv/crawl4ai-mcp-llm)** as your crawler to fetch and snapshot live documentation into structured Markdown, then use **`md-doc-search`** for instant (~40 ms), zero-token, offline retrieval.

## Preparing corpora

Any Markdown file works as-is. However, raw documentation crawled from the web or exported from official manuals often contains boilerplate or unindexed formatting that impairs search quality. Here is how real-world corpora are retrieved and prepared step-by-step.

### 1. Sourcing the documentation

There are two primary ways to obtain a full documentation snapshot:

#### Method A: Official Offline Exports (e.g. EPUB via Pandoc)
Projects like Blender publish offline EPUB or HTML archives for each LTS release.
1. Download the versioned EPUB, e.g. `https://docs.blender.org/manual/en/5.2/blender_manual_epub.zip` (Blender wraps the EPUB in a zip).
2. Convert it into a single consolidated Markdown file with media assets extracted locally using [Pandoc](https://pandoc.org/):
   ```bash
   pandoc -f epub -t markdown --extract-media=media blender_manual_v5.2_en.epub -o Blender52LTSManual.md
   ```
   *Result:* A single ~11 MB Markdown file with all 3,000+ screenshots saved under `media/` for visual AI citations.

One command does all of it (download, EPUB unwrapping, pandoc conversion, and extraction of every screenshot stored in the EPUB — pandoc alone extracts only AST-referenced media, leaving the raw-HTML `<img>` screenshots dangling):
```bash
python scripts/fetch_docs.py blender --epub https://docs.blender.org/manual/en/5.2/blender_manual_epub.zip
```

#### Method B: AI-Driven Web Crawl via [crawl4ai-mcp-llm](https://github.com/laurentvv/crawl4ai-mcp-llm) — *Recommended for online docs*
When documentation is only available online (e.g. Godot, Ansible, frameworks):

Use **[crawl4ai-mcp-llm](https://github.com/laurentvv/crawl4ai-mcp-llm)**, an MCP (Model Context Protocol) server designed specifically to allow AI assistants to crawl websites, bypass anti-bots (Cloudflare, CAPTCHAs via Magic Mode), render client-side SPAs, and extract clean, structured Markdown ready for offline search.

- **Instant execution via `uvx`** (no manual installation required):
  ```bash
  uvx --python 3.13 crawl4ai-mcp-llm
  ```
- **Agent Integration** (Claude Desktop, Cursor, Antigravity, Cline):
  Add to your AI assistant's MCP configuration (`cline_mcp_settings.json` or `claude_desktop_config.json`):
  ```json
  {
    "mcpServers": {
      "crawl": {
        "command": "uvx",
        "args": ["--python", "3.13", "crawl4ai-mcp-llm"]
      }
    }
  }
  ```
- **Workflow**:
  1. Instruct your agent to crawl the targeted version tree (e.g. `https://docs.godotengine.org/en/4.7/` or `https://docs.ansible.com/`).
  2. `crawl4ai-mcp-llm` extracts high-quality Markdown, traverses documentation links, and prepends canonical URLs (`> Source: <url>`).
  3. Concatenate the output into a single version-pinned Markdown file (`godot_docs_stable_47.md`).
  4. Normalize the result: `python scripts/fetch_docs.py godot` — repairs fences broken by crawler truncation and strips the per-page navigation scaffolding (see measured impact in the next section).

#### Method C: Official `llms.txt` Indexes (e.g. Roblox) — *no crawler needed*
When a project publishes an `llms.txt` index of its raw Markdown pages, prefer it over crawling: it is a plain parallel download. Roblox does (`create.roblox.com/docs/llms.txt` for guides, `create.roblox.com/docs/reference/engine/llms.txt` for the Engine API). The bundled script downloads every listed `.md` page in parallel (stdlib only — no crawler, no anti-bot layer), strips frontmatter, concatenates the pages in index order, then normalizes:

```bash
python scripts/fetch_docs.py roblox-docs        # 2 280 guide pages
python scripts/fetch_docs.py roblox-engine      # 1 194 Engine API pages
```

---

### 2. Post-processing & normalization

Raw web crawls contain broken code fences, navigation boilerplate and unindexed formatting that silently corrupt section slicing — the measured impact on real corpora: 62 % of "headings" were code comments inside fences (Godot), and one unclosed fence can swallow every page after it. **`scripts/fetch_docs.py` automates retrieval and normalization**:

```bash
python scripts/fetch_docs.py roblox-docs                                   # fetch via official llms.txt + normalize
python scripts/fetch_docs.py roblox-engine
python scripts/fetch_docs.py blender --epub https://docs.blender.org/manual/en/5.2/blender_manual_epub.zip  # download + pandoc + screenshots
python scripts/fetch_docs.py godot                                         # normalize the existing crawled corpus
python scripts/fetch_docs.py godot --dry-run                               # stats only
python scripts/fetch_docs.py roblox-docs --input docs/roblox/roblox_docs.md  # normalize without re-fetching
```

It writes `<name>_clean.md` next to the original (never touches it), refuses to write if fences remain unbalanced, and reports before/after statistics. The normalization rules, measured on the bundled corpora:

| Rule | What it fixes | Measured impact |
|---|---|---|
| **Fence healing** (R1) | fences left open by crawler truncation (`[Content truncated due to length]` markers, page starts, `Copy to clipboard` anchors) — a missing closer inverts open/close roles for everything downstream | Godot: 2 truncation points desynchronized 172 000 lines; 1 590 trapped pages freed, headings visible went 7 664 → 16 238 |
| **Glued-fence repair** (R2) | ```` ``` ```` runs glued mid-line (Roblox table cells, `:```bash` list items) and uncloseable 4-6 backtick openers (` ``````bash `) | Roblox docs: 656 fences repaired, headings visible 958 → 9 065, a tutorial page trapped in a phantom fence released |
| **Scaffolding strip** (R3) | per-page nav menu, breadcrumb, `Copy to clipboard`, `* * *` separators, `User-contributed notes` footers, duplicate suffixed H1 | Godot: ~87 400 lines removed (34 %), ~9 ms faster queries |

Manual equivalents (if you process another corpus by hand): strip repeated nav blocks that pollute IDF; convert `# Source: <url>` to `> Source: <url>` blockquotes so links stay citable without creating competing headings. The old "promote `**method**(` to `###`" recipe turned out to be unnecessary for the current Godot 4.7 corpus (methods are already `###` sections; 0 bold signatures found) — check before applying it.

Relevance regression suite:

```bash
python scripts/relevance_check.py   # 39 assertions across all four corpora
```

## Limitations

- **Full-text, not semantic** — queries need the corpus' vocabulary. That is precisely where agents need grounding (exact identifiers), but for concept-level questions pair it with your agent's general knowledge.
- **Linear scan** — instant up to ~100 MB corpora; beyond that, port the scoring to an indexed engine (e.g. Tantivy).
- Output section markers and truncation messages are in French (the author's language, kept stable for existing skills); scores and content are untouched corpus text, and errors go to stderr in English.
- A Setext underline (`---`) directly following a list item line is treated as a heading — a rare markdown edge case.

## Development

```bash
cargo test                                          # 40 unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
python scripts/relevance_check.py                   # relevance suite on local corpora (docs/, gitignored)
```

CI runs fmt, clippy, tests and a release build on windows / linux / macos. Publishing a GitHub release (tag `vX.Y.Z`) also attaches prebuilt binaries automatically (`windows-x64`, `linux-x64`, `macos-arm64`, `macos-x64` — see `.github/workflows/release.yml`).

## License

MIT — see [LICENSE](LICENSE). Documentation corpora you search remain under their own licenses (Blender docs: CC-BY; Godot docs: CC-BY) — this repository ships the *tool*, not the corpora.
