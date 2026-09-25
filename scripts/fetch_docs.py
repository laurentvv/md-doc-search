#!/usr/bin/env python3
"""Fetch and normalize Markdown documentation corpora for md-doc-search.

Retrieval methods (mirroring the pipelines used across C:/GIT repos, see
README "Preparing corpora"):
  roblox-docs / roblox-engine  official llms.txt indexes -> parallel download
                               of the raw .md pages -> concatenation
                               (stdlib only; port of roblox/tools/build_roblox_doc_corpus.py)
  blender                      official EPUB -> pandoc -> single .md (+ media/)
                               (the URL may be the EPUB itself or Blender's
                               blender_manual_epub.zip wrapper; unwrapped
                               automatically)
  godot                        normalize-only: the crawled corpus already on
                               disk (fetching pipeline involves crawl4ai-mcp +
                               godot --doctool XML re-injection; see
                               docs/project/project_bible.md)

Normalization is deterministic and fence-aware (CommonMark-style tracking,
same rules as the Rust parser):
  R1  close code fences left open by crawler truncation
      (Godot "...[Content truncated due to length]..." markers)
  R2  move mid-line ``` fences onto their own line
      (Roblox tables embed fences inside cells, which breaks fence parity
      and swallows real content)
  R3  strip repeated navigation/footer scaffolding from every Godot page
      (nav menu, breadcrumb, "Copy to clipboard", "* * *", "User-contributed
      notes" footer, duplicate suffixed H1, thematic breaks)

Outputs: the raw corpus (when fetched) plus a `<name>_clean.md` next to it.
Originals are never modified. Every run prints fence-parity and heading
statistics before/after.

Examples:
  python scripts/fetch_docs.py roblox-docs
  python scripts/fetch_docs.py roblox-engine
  python scripts/fetch_docs.py blender --epub \
      https://docs.blender.org/manual/en/5.2/blender_manual_epub.zip  # or a local path
  python scripts/fetch_docs.py godot            # normalize the existing corpus
  python scripts/fetch_docs.py godot --dry-run  # stats only, write nothing
"""

import argparse
import concurrent.futures
import datetime
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DOCS = REPO / "docs"
USER_AGENT = "md-doc-search-fetch/0.2 (+https://github.com/laurentvv/md-doc-search)"

TRUNCATION_MARKER = "[Content truncated due to length]"
GODOT_NAV_LINES = {
    "About", "Getting started", "Manual", "Engine details", "Community",
    "Class reference",
}
GODOT_BREADCRUMB_PREFIX = " * Godot Engine 4.7 documentation in English"
GODOT_H1_SUFFIX_RE = re.compile(r"^#\s.* — Godot Engine \(stable\) documentation in English\s*$")
HEADING_RE = re.compile(r"^ {0,3}#{1,3} \S")

# ---------------------------------------------------------------- fence model


class FenceTracker:
    """Fence state machine faithful to src/parse.rs: a closer must use the
    same character and be at least as long as the opener, with nothing else
    on the line."""

    def __init__(self):
        self.open = None  # (ch, length) of the currently open fence

    def update(self, line: str):
        """Returns (opens, closes) for this line."""
        stripped = line.lstrip(" ")
        if len(line) - len(stripped) > 3:
            return False, False
        s = stripped.rstrip("\r\n")
        if s[:3] not in ("```", "~~~"):
            return False, False
        ch = s[0]
        length = len(s) - len(s.lstrip(ch))
        if length < 3:
            return False, False
        rest = s[length:]
        if ch == "`" and "`" in rest:
            return False, False
        if self.open is not None:
            och, olen = self.open
            if ch == och and length >= olen and rest.strip() == "":
                self.open = None
                return False, True
            return False, False
        self.open = (ch, length)
        return True, False


def stats(lines):
    """(fence_parity_ok, headings_outside_fences) for a report."""
    tracker = FenceTracker()
    headings = 0
    for line in lines:
        opens, closes = tracker.update(line)
        if opens or closes:
            continue
        if tracker.open is None and HEADING_RE.match(line):
            headings += 1
    return tracker.open is None, headings


# ------------------------------------------------------------ normalizations


def heal_fences(lines):
    """R1: restore fence parity using three anchors that must sit OUTSIDE a
    code block in this corpus family:
      a) crawler truncation markers (the code block ends there),
      b) page starts ("> Source: http..." / suffixed H1 title),
      c) "Copy to clipboard" artifacts (always follow a real fence closer).
    A missing closer inverts open/close roles for everything downstream:
    code comments become phantom headings and whole pages get fenced. Each
    anchor re-closes the tracked fence, healing at the next structural point.
    """
    out = []
    tracker = FenceTracker()
    counts = {"marker": 0, "page-start": 0, "clipboard": 0}

    for line in lines:
        stripped = line.strip()
        page_start = stripped.startswith("> Source: http") or (
            stripped.startswith("# ") and " — Godot Engine" in stripped)
        is_marker = TRUNCATION_MARKER in line
        is_clipboard = stripped == "Copy to clipboard"

        if tracker.open is not None:
            if is_marker or page_start or is_clipboard:
                out.append("```\n")  # lines keep their endings: add ours
                tracker.open = None
                counts["marker" if is_marker else
                       "page-start" if page_start else "clipboard"] += 1
                out.append(line)
                continue
            tracker.update(line)
            out.append(line)
            continue
        out.append(line)
        tracker.update(line)
    return out, counts


def fix_midline_fences(lines):
    """R2 (state-independent rewrites): repair fences the crawler glued:
      - a line-start run of 4+ backticks (e.g. "``````bash") can never be
        closed by the plain ``` closers present in the file: collapse to 3;
      - a ``` run sitting mid-line (table cells, list items ending in
        ":```bash") is moved onto its own line so openers/closers pair up.
    Tracking state across edits is a fixpoint chase (each rewrite shifts the
    fence state seen by later lines), so these rewrites are purely syntactic;
    the strict parity check in process() validates the result.
    """
    out, n = [], 0
    for line in lines:
        stripped = line.rstrip("\r\n")
        indent = len(stripped) - len(stripped.lstrip(" "))
        if indent > 3:
            out.append(line)
            continue
        body = stripped[indent:]
        run = len(body) - len(body.lstrip("`"))
        if run >= 3:
            rest = body[run:]
            if run > 3 and not rest.startswith(("`", "|")):
                # 4+ backticks: collapse so plain ``` closers can pair.
                out.append(" " * indent + "```" + rest + "\n")
                n += 1
                continue
            out.append(line)
            continue
        idx = body.find("```")
        if idx > 0:
            fence = body[idx:]
            fence_run = len(fence) - len(fence.lstrip("`"))
            if fence_run > 3:
                fence = "```" + fence.lstrip("`")
            if "|" in fence:  # table-cell closer like "``` |" -> bare closer
                fence = fence.split("|")[0].rstrip()
            prefix = (" " * indent + body[:idx]).rstrip()
            out.append(prefix + "\n")
            out.append(fence + "\n")
            n += 1
            continue
        out.append(line)
    return out, n


def strip_godot_scaffolding(lines):
    """R3: drop the per-page navigation/footer boilerplate (outside fences)."""
    out, counts = [], {}
    tracker = FenceTracker()
    drop_footer = False
    drop_breadcrumb = False
    footer_len = 0

    def drop(kind):
        counts[kind] = counts.get(kind, 0) + 1

    for line in lines:
        stripped = line.strip()
        s = line.lstrip(" ")
        is_fence = (len(line) - len(s) <= 3) and s[:3] in ("```", "~~~")

        if tracker.open is not None:
            tracker.update(line)
            out.append(line)
            continue
        if is_fence:
            tracker.update(line)
            out.append(line)
            continue

        if drop_footer:
            # Footer blocks are short; a truncated one may lack its
            # "Hosted by" terminator. Consume the terminator, never swallow
            # structural lines, and cap the drop so a missing terminator
            # cannot cascade.
            if stripped.startswith("Hosted by "):
                drop_footer = False
                drop("footer")
                continue
            if is_fence or stripped == "---" or stripped.startswith("> Source:") \
                    or footer_len > 40 or stripped.startswith("#"):
                drop_footer = False
                out.append(line)
                continue
            footer_len += 1
            drop("footer")
            continue

        if drop_breadcrumb:
            drop_breadcrumb = False
            if stripped.startswith("* "):
                drop("breadcrumb")
                continue

        if stripped in GODOT_NAV_LINES:
            drop("nav")
        elif stripped.startswith(GODOT_BREADCRUMB_PREFIX):
            drop("breadcrumb")
            drop_breadcrumb = True
        elif stripped == "Copy to clipboard":
            drop("copy-to-clipboard")
        elif stripped == "* * *":
            drop("separator")
        elif stripped == "---":
            drop("thematic-break")
        elif GODOT_H1_SUFFIX_RE.match(line):
            drop("h1-suffix")
        elif stripped == "## User-contributed notes":
            drop_footer = True
            footer_len = 0
            drop("footer")
        elif line.rstrip("\r\n") != "" and stripped == "":
            drop("whitespace-only")  # spaces-only line, not a blank line
        else:
            out.append(line)
    return out, counts


def normalize(name: str, lines, report):
    if name.startswith("godot"):
        lines, counts = heal_fences(lines)
        report("R1 fence healing: " + ", ".join(
            f"{v}x {k}" for k, v in counts.items() if v))
        lines, counts = strip_godot_scaffolding(lines)
        report("R3 scaffolding removed: " + ", ".join(
            f"{v}x {k}" for k, v in sorted(counts.items(), key=lambda kv: -kv[1])))
    elif name.startswith("roblox"):
        lines, n = fix_midline_fences(lines)
        report(f"R2 moved {n} mid-line fence(s) onto their own line")
    else:
        report("no normalization for this corpus")
    return lines


# ------------------------------------------------------------------ fetching

LINK_RE = re.compile(r"\((/docs[^)]*\.md)\)")
FRONTMATTER_RE = re.compile(r"\A---\s*\n.*?\n---\s*\n", re.DOTALL)


def http_get(url: str, tries: int = 3) -> bytes:
    for attempt in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=60) as resp:
                return resp.read()
        except Exception as e:  # noqa: BLE001 - report after last try
            if attempt == tries - 1:
                raise
            print(f"  retry {attempt + 1}/{tries} after error: {e}", file=sys.stderr)
            time.sleep(2 * (attempt + 1))
    raise RuntimeError("unreachable")


def http_download(url: str, dest: Path):
    """Stream a large file to disk (the Blender EPUB zip is ~500 MB)."""
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=60) as resp, open(dest, "wb") as f:
        shutil.copyfileobj(resp, f)
    print(f"downloaded {dest.stat().st_size / 1e6:.0f} MB -> {dest}")


def unwrap_epub(pkg: Path) -> Path:
    """Return the file pandoc should read. An EPUB *is* a zip (it carries
    META-INF/container.xml); Blender ships the EPUB wrapped in a second zip
    (blender_manual_epub.zip), which pandoc cannot read — extract the .epub
    member next to it."""
    if not zipfile.is_zipfile(pkg):
        return pkg
    with zipfile.ZipFile(pkg) as z:
        names = z.namelist()
    if "META-INF/container.xml" in names:
        return pkg
    members = [n for n in names if n.lower().endswith(".epub")]
    if len(members) != 1:
        return pkg  # not something we can fix; pandoc will report it
    inner = pkg.with_name(pkg.stem + ".epub")
    if inner == pkg:
        inner = pkg.with_suffix(".unwrapped.epub")
    with zipfile.ZipFile(pkg) as z, z.open(members[0]) as src, \
            open(inner, "wb") as dst:
        shutil.copyfileobj(src, dst)
    print(f"unwrapped {members[0]} from {pkg.name} -> {inner.name}")
    return inner


def fetch_roblox(index_path: str, out_md: Path):
    """Download all .md pages listed in the official llms.txt index."""
    base = "https://create.roblox.com"
    index = http_get(f"{base}/{index_path}").decode("utf-8", errors="replace")
    links = sorted(set(LINK_RE.findall(index)))
    if len(links) < 100:
        sys.exit(f"abort: only {len(links)} pages found in llms.txt (expected hundreds)")

    def get(url):
        try:
            return url, http_get(url).decode("utf-8", errors="replace")
        except Exception as e:  # noqa: BLE001 - collect failures
            return url, e

    pages, failures = {}, []
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        for url, result in pool.map(get, (f"{base}{path}" for path in links)):
            if isinstance(result, Exception):
                failures.append((url, result))
            else:
                pages[url] = result
    if failures:
        print(f"  WARNING: {len(failures)} page(s) failed, e.g. {failures[0]}", file=sys.stderr)

    parts = []
    for path in links:  # document order from the index
        text = pages.get(f"{base}{path}")
        if text is None:
            continue
        parts.append(FRONTMATTER_RE.sub("", text, count=1).rstrip() + "\n\n")
    out_md.parent.mkdir(parents=True, exist_ok=True)
    out_md.write_text("".join(parts), encoding="utf-8", newline="")
    (out_md.parent / (out_md.stem + ".pages.txt")).write_text(
        "\n".join(links), encoding="utf-8")
    print(f"fetched {len(pages)}/{len(links)} pages -> {out_md}")


EPUB_MEDIA_EXTS = (".png", ".jpg", ".jpeg", ".webp", ".svg", ".gif")


def extract_epub_media(epub_path: Path, media_dir: Path):
    """Copy every image stored in the EPUB into media/, preserving its path.

    pandoc only extracts media referenced from its AST (title-page images):
    the manual's screenshots sit in raw-HTML <img> blocks whose src pandoc
    rewrites but whose files it never writes out, so without this step every
    screenshot reference in the corpus is dangling."""
    extracted = 0
    with zipfile.ZipFile(epub_path) as z:
        for info in z.infolist():
            parts = info.filename.split("/")
            if parts[-1].lower().endswith(EPUB_MEDIA_EXTS) and ".." not in parts:
                dest = media_dir.joinpath(*parts)
                dest.parent.mkdir(parents=True, exist_ok=True)
                with z.open(info) as src, open(dest, "wb") as dst:
                    shutil.copyfileobj(src, dst)
                extracted += 1
    print(f"extracted {extracted} media file(s) -> {media_dir}")


def fetch_blender(epub: str, out_md: Path):
    """EPUB (URL or local path) -> pandoc -> markdown, media under media/."""
    if not shutil.which("pandoc"):
        sys.exit("abort: pandoc not found in PATH (see README 'Method A')")
    work = out_md.parent
    work.mkdir(parents=True, exist_ok=True)
    if epub.startswith(("http://", "https://")):
        print(f"downloading {epub} ...")
        pkg = work / (epub.rsplit("/", 1)[-1].split("?")[0] or "manual.epub")
        http_download(epub, pkg)
    else:
        pkg = Path(epub).resolve()
        if not pkg.exists():
            sys.exit(f"abort: EPUB not found: {pkg}")
    epub_path = unwrap_epub(pkg)
    cmd = ["pandoc", "-f", "epub", "-t", "markdown", "--extract-media=media",
           str(epub_path), "-o", str(out_md)]
    subprocess.run(cmd, check=True, cwd=work)
    extract_epub_media(epub_path, work / "media")
    print(f"pandoc ok -> {out_md} (media under {work / 'media'})")


# ---------------------------------------------------------------------- main


def process(name: str, raw: Path, clean: Path, dry_run: bool):
    lines = raw.read_text(encoding="utf-8", errors="replace").splitlines(keepends=True)
    parity0, headings0 = stats(lines)
    print(f"before: {len(lines)} lines, fences balanced: {parity0}, "
          f"H1-H3 outside fences: {headings0}")

    if dry_run:
        print("dry-run: no file written")
        return
    lines = normalize(name, lines, lambda msg: print("  " + msg))
    parity1, headings1 = stats(lines)
    print(f"after:  {len(lines)} lines, fences balanced: {parity1}, "
          f"H1-H3 outside fences: {headings1}")
    if not parity1:
        sys.exit("abort: fences still unbalanced after normalization "
                 "(would corrupt section slicing); nothing written")
    clean.write_text("".join(lines), encoding="utf-8", newline="")
    print(f"-> {clean}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("corpus", choices=["roblox-docs", "roblox-engine", "blender", "godot"])
    ap.add_argument("--epub", help="blender only: EPUB URL or local path")
    ap.add_argument("--input", type=Path, help="normalize-only: source file "
                                               "(default: the repo's copy)")
    ap.add_argument("--dry-run", action="store_true", help="stats only, write nothing")
    args = ap.parse_args()

    stamp = datetime.date.today().isoformat()
    if args.input:  # normalize an existing file, no fetching
        raw = args.input
    elif args.corpus == "roblox-docs":
        raw = DOCS / "roblox" / "roblox_docs.md"
        fetch_roblox("docs/llms.txt", raw)
    elif args.corpus == "roblox-engine":
        raw = DOCS / "roblox" / "roblox_engine_api.md"
        fetch_roblox("docs/reference/engine/llms.txt", raw)
    elif args.corpus == "blender":
        if not args.epub:
            sys.exit("blender requires --epub <url-or-path>")
        raw = DOCS / "blender" / "Blender52LTSManual.md"
        fetch_blender(args.epub, raw)
    else:  # godot: normalize-only
        raw = DOCS / "godot" / "godot_docs_stable_47.md"

    if args.corpus != "blender" and not args.dry_run:
        clean = raw.with_name(raw.stem + "_clean.md")
        process(args.corpus, raw, clean, args.dry_run)
    elif args.dry_run:
        process(args.corpus, raw, raw.with_name(raw.stem + "_clean.md"), True)
    (raw.parent / "BUILT_AT.txt").write_text(
        f"{args.corpus}: raw fetched/normalized {stamp}\n", encoding="utf-8")


if __name__ == "__main__":
    main()
