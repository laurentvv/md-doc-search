![md-doc-search banner](assets/header.jpg)

# md-doc-search

**Offline full-text search over version-pinned Markdown documentation — built to ground AI coding agents on the exact docs version they target.**

![Rust](https://img.shields.io/badge/rust-1.85%2B-orange) ![License](https://img.shields.io/badge/license-MIT-blue) ![Platform](https://img.shields.io/badge/platform-windows%20%7C%20linux%20%7C%20macos-lightgrey)

A single ~150-line Rust binary that answers *precise* technical queries (`Boolean Modifier`, `Input is_action_just_pressed`, `Volume Scatter`) against a local Markdown snapshot of the documentation you actually target — in ~40 ms, with no network, no server, and no index to maintain.

## The problem it solves

LLM coding agents hallucinate APIs, and web documentation makes it worse: search results blend every version ever published. When your agent writes code against **Blender 5.2** or **Godot 4.7**, it needs *that* version's docs — not a blend of 2022 Stack Overflow answers.

`md-doc-search` turns a documentation snapshot into a deterministic, citation-friendly lookup tool your agent can call from a shell:

```text
$ md-doc-search godot_docs.md "Input is_action_just_pressed" 500
Résultats de recherche pour 'Input is_action_just_pressed' :

--- DÉBUT DE SECTION (Score: 156) ---
### is_action_just_pressed(action: , exact_match: = false) const
Returns `true` when the user has started pressing the action event...
--- FIN DE SECTION ---
```

Real-world corpora we run daily through this exact binary:

| Corpus | Size | Content | Query time |
|---|---|---|---|
| Blender 5.2 LTS manual | 11 MB | full manual, 3 116 referenced screenshots | ~57 ms |
| Godot 4.7 stable docs | 10.7 MB | 1 076 class pages + 392 tutorials | ~40 ms |

## Install

```bash
git clone https://github.com/laurentvv/md-doc-search
cd md-doc-search
cargo build --release
# binary: target/release/md-doc-search(.exe)
# or system-wide:
cargo install --path .
```

Requires Rust 1.85+ (edition 2024). Only dependency: `regex`.

## Usage

```bash
md-doc-search <markdown_file> "<keywords>" [max_tokens] [top_k]
```

| Argument | Default | Description |
|---|---|---|
| `markdown_file` | — | path to any Markdown corpus |
| `keywords` | — | query, as a single argument (spaces allowed, **English keywords** for English docs) |
| `max_tokens` | `8000` | output budget (1 token ≈ 4 chars); sections are truncated to fit |
| `top_k` | `3` | number of sections returned |

Try it immediately on the bundled sample:

```bash
md-doc-search examples/demo.md "boolean modifier"
```

## How ranking works

Sections are split on `#`, `##` and `###` headings, then scored:

1. **IDF-weighted keywords** — terms rare across the corpus (`boolean`, `anisotropy`) weigh far more than ubiquitous ones (`modifier`, `options`). This alone kills the classic failure where generic "## Options" boilerplate outranks the page you want.
2. **Page-title inheritance** — every section remembers the `#` H1 of the page that carries it. A `## Options` section under `# Boolean Modifier` gets credit for its page, so it ranks as *Boolean context*, never as orphaned boilerplate.
3. **Heading bonuses** — matches in the section heading (+15 × weight) and owning page title (+10 × weight); ×1.4 bonus when *all* query terms appear in the heading chain.
4. **Exact-phrase bonus** for the full query string.
5. **Occurrence cap (20)** and **length normalization** — giant sections can't win by being giant.
6. **Token budget** — output is truncated per section to respect `max_tokens`, with an explicit truncation marker (agents can raise the budget and re-query).

Sort is deterministic (score, then document order).

## Grounding your AI agent

The intended deployment is a tiny agent *skill* per corpus: a `SKILL.md` that hard-codes the corpus path, mandates English keywords, and tells the agent how to read the output. Minimal example:

```markdown
---
name: blender-doc-search
description: Search the local Blender 5.2 LTS manual. Use whenever a Blender
  feature, operator, modifier, node, shortcut or Python API question comes up.
---
# Blender Doc Search
"C:/GIT/md-doc-search/target/release/md-doc-search.exe" \
  "C:/corpora/Blender52LTSManual.md" "<english keywords>" 800
```

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
1. Download the versioned EPUB (e.g. `blender_manual_v5.2_en.epub` from `docs.blender.org`).
2. Convert it into a single consolidated Markdown file with media assets extracted locally using [Pandoc](https://pandoc.org/):
   ```bash
   pandoc -f epub -t markdown --extract-media=media blender_manual_v5.2_en.epub -o Blender52LTSManual.md
   ```
   *Result:* A single ~11 MB Markdown file with all 3,000+ screenshots saved under `media/` for visual AI citations.

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

---

### 2. Post-processing & cleanup recipes

Raw web crawls and exports contain navigation bars, footers, and unindexed formatting. Applying these simple regex transformations measurably improves search accuracy:

#### A. Strip crawler scaffolding and navigation boilerplate
Repeated navigation menus, sidebars, breadcrumbs, and search box templates pollute keyword frequency (IDF) and clutter sections.
- Remove repeating blocks such as:
  ```text
  About | Getting started | Manual | Engine details | Community | Class reference
  ```
  *(Cleaning up crawler boilerplate eliminated 1,590 junk sections in the Godot corpus).*

#### B. Promote method signatures to H3 headings (`###`)
The search engine splits sections on `(?m)^#{1,3}\s`. In API documentation, method definitions are often formatted in bold markdown instead of actual headings:
- **Transform:** `**method_name**(` &rarr; `### method_name(`
- **Example regex replacement (Python / sed / editor):**
  - **Find:** `(?m)^\*\*([a-zA-Z0-9_]+)\*\*\((.*)`
  - **Replace:** `### $1($2`
  *(In the Godot corpus, this created 6,391 dedicated method sections, moving the exact method directly to Rank #1).*

#### C. Demote pure-link headings to blockquotes
If a crawler outputs `# Source: https://...` or `# https://...` as an H1, the scoring algorithm treats the link as a page heading and splits on it:
- **Transform:** `# Source: <url>` &rarr; `> Source: <url>`
- **Example regex replacement:**
  - **Find:** `(?m)^#\s+(Source:\s+https?://\S+)`
  - **Replace:** `> $1`
  *(This keeps the URL accessible and citable by the LLM without creating an empty competing heading).*

## Limitations

- **Full-text, not semantic** — queries need the corpus' vocabulary. That is precisely where agents need grounding (exact identifiers), but for concept-level questions pair it with your agent's general knowledge.
- **Linear scan** — instant up to ~100 MB corpora; beyond that, port the scoring to an indexed engine (e.g. Tantivy).
- Output messages and truncation markers are in French (the author's language); scores and content are untouched corpus text.

## License

MIT — see [LICENSE](LICENSE). Documentation corpora you search remain under their own licenses (Blender docs: CC-BY; Godot docs: CC-BY) — this repository ships the *tool*, not the corpora.
