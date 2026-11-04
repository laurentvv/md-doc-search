#!/usr/bin/env python3
"""Relevance measurement for directory corpora (stdlib only).

Runs md-doc-search on each case of a relevance set (default:
scripts/relevance/doc-en-ligne.json) against its docset directory and reports
hit@1, hit@3, hit@5 and MRR@5, per docset and per query kind. A hit is a result
block whose `> File:` page matches one of the case's expected pages (exact
relative path, or a prefix ending with '/' or '-'), or has the same content as
one of them (front matter excluded): mirrors ship verbatim copies such as
man-page aliases, and returning a copy returns the same text.

The corpora are not shipped with this repository: pass their root with --root
or the DOC_EN_LIGNE_ROOT environment variable. Every expected page is checked
to exist before anything runs, so a renamed page fails loudly instead of
silently counting as a miss.

Usage:
  python scripts/directory_relevance.py --root /path/to/doc-en-ligne
  python scripts/directory_relevance.py --root ... --show-misses
  python scripts/directory_relevance.py --root ... --json results.json
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_SET = REPO / "scripts" / "relevance" / "doc-en-ligne.json"
TOP_K = 5


def default_binary() -> Path:
    name = "md-doc-search.exe" if os.name == "nt" else "md-doc-search"
    return REPO / "target" / "release" / name


def matches(page: str, expected: str) -> bool:
    if expected.endswith(("/", "-")):
        return page.startswith(expected) or f"/{expected}" in f"/{page}"
    return page == expected or page.endswith(f"/{expected}")


def pages_of(docset_dir: Path) -> list[str]:
    return [p.relative_to(docset_dir).as_posix() for p in docset_dir.rglob("*.md")]


def content_key(path: Path) -> str:
    """Hash of a page without its YAML front matter (copies differ only there)."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    if lines and lines[0].strip() == "---":
        end = next((i for i in range(1, len(lines)) if lines[i].strip() == "---"), None)
        if end is not None:
            lines = lines[end + 1:]
    text = "\n".join(lines)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def files_in_output(stdout: str) -> list[str]:
    return [line[len("> File: "):].strip() for line in stdout.splitlines() if line.startswith("> File: ")]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", default=os.environ.get("DOC_EN_LIGNE_ROOT"), help="root of the docset folders")
    parser.add_argument("--set", default=str(DEFAULT_SET), help="relevance set (JSON)")
    parser.add_argument("--bin", default=str(default_binary()), help="md-doc-search binary")
    parser.add_argument("--show-misses", action="store_true", help="print the pages returned for each miss")
    parser.add_argument("--json", help="write per-case results to this file")
    args = parser.parse_args()
    if not args.root:
        parser.error("--root (or DOC_EN_LIGNE_ROOT) is required")
    root = Path(args.root)
    binary = str(Path(args.bin).resolve())  # Windows does not resolve a relative executable against the cwd
    cases = json.loads(Path(args.set).read_text(encoding="utf-8"))["cases"]

    pages_cache: dict[str, list[str]] = {}
    missing = []
    for case in cases:
        docset_dir = root / case["docset"]
        if case["docset"] not in pages_cache:
            pages_cache[case["docset"]] = pages_of(docset_dir) if docset_dir.is_dir() else []
        for expected in case["expected"]:
            if not any(matches(p, expected) for p in pages_cache[case["docset"]]):
                missing.append(f"{case['docset']}: {expected}")
    if missing:
        print("Expected pages not found (fix the set or the root):", *missing, sep="\n  ", file=sys.stderr)
        return 2

    results = []
    t0 = time.perf_counter()
    for case in cases:
        proc = subprocess.run(
            # `--` ends the options: queries such as "--deploy-hook reload" are positional.
            [binary, "--top-k", str(TOP_K), "--max-tokens", "100000", "--", str(root / case["docset"]),
             case["query"]],
            capture_output=True, text=True, encoding="utf-8")
        if proc.returncode not in (0, 1):
            print(f"error on {case['query']!r}: {proc.stderr.strip()}", file=sys.stderr)
            return 2
        files = files_in_output(proc.stdout)
        docset_dir = root / case["docset"]
        wanted = {content_key(docset_dir / p) for p in pages_cache[case["docset"]]
                  if any(matches(p, e) for e in case["expected"]) and not any(e.endswith(("/", "-"))
                                                                              for e in case["expected"])}

        def good(page: str) -> bool:
            if any(matches(page, e) for e in case["expected"]):
                return True
            return bool(wanted) and content_key(docset_dir / page) in wanted

        rank = next((i + 1 for i, f in enumerate(files) if good(f)), None)
        results.append({**case, "rank": rank, "returned": files})
    elapsed = time.perf_counter() - t0

    def summary(rows: list[dict]) -> str:
        n = len(rows)
        hit = lambda k: sum(1 for r in rows if r["rank"] and r["rank"] <= k)  # noqa: E731
        mrr = sum(1 / r["rank"] for r in rows if r["rank"]) / n
        return f"| {n} | {hit(1)} | {hit(3)} | {hit(5)} | {mrr:.3f} |"

    print("| group | cases | hit@1 | hit@3 | hit@5 | MRR@5 |")
    print("|---|---|---|---|---|---|")
    for docset in dict.fromkeys(r["docset"] for r in results):
        print(f"| {docset.split('/')[0]} " + summary([r for r in results if r["docset"] == docset]))
    for kind in ("identifier", "task"):
        print(f"| *{kind}* " + summary([r for r in results if r["kind"] == kind]))
    print("| **all** " + summary(results))
    print(f"\n{len(results)} queries in {elapsed:.1f} s ({elapsed / len(results) * 1000:.0f} ms per query)")

    if args.show_misses:
        for r in results:
            if not r["rank"] or r["rank"] > 1:
                print(f"\n[{r['rank'] or '-'}] {r['docset'].split('/')[0]} « {r['query']} » expected {r['expected']}")
                for f in r["returned"]:
                    print(f"    {f}")
    if args.json:
        Path(args.json).write_text(json.dumps(results, ensure_ascii=False, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
