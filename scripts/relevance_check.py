#!/usr/bin/env python3
"""Relevance benchmark for md-doc-search.

Runs a fixed set of queries against local corpora through the release binary
and checks expectations:
  rank1 <substr>  the best-scoring returned section contains <substr>
  top3  <substr>  one of the first three returned sections contains <substr>
  found           the query must return results (exit code 0)

Exit code is non-zero if any expectation fails. Missing corpora (e.g. the
`*_clean.md` files before `clean_corpus.py --write`) are skipped with a notice.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

# Words that only exist on bare headings of the (broken) Godot corpus:
# before the bare-heading fix every one of them was silently unfindable.
GODOT_LOST_WORDS = [
    "lewis", "carroll", "jabberwocky", "tutorial_1", "tutorial_2",
    "the_plugin", "timestamped", "handle_crash", "discriminator", "core_bind",
    "os_linuxbsd", "__libc_start_main", "0x4da3358", "0x7f6e5e260575",
    "6c9aa4c7d3b9b91cd50714c40eeb234874df7075",
]

CASES = [
    # --- demo corpus ---
    ("examples/demo.md", "options", "found", None),
    ("examples/demo.md", "merge_threshold", "rank1", "merge_threshold"),
    ("examples/demo.md", "anisotropy", "rank1", "Volume Scatter"),
    # --- Godot (original, uncleaned) ---
    ("docs/godot/godot_docs_stable_47.md", "Input is_action_just_pressed", "rank1", "is_action_just_pressed"),
    ("docs/godot/godot_docs_stable_47.md", "CharacterBody2D move_and_slide", "rank1", "move_and_slide"),
    ("docs/godot/godot_docs_stable_47.md", "speed between 0 and 20", "top3", "Color8"),
    ("docs/godot/godot_docs_stable_47.md", "StandardMaterial3D", "found", None),
    ("docs/godot/godot_docs_stable_47.md", "resource preloader", "found", None),
    # --- Godot (cleaned) ---
    ("docs/godot/godot_docs_stable_47_clean.md", "StandardMaterial3D", "rank1", "StandardMaterial3D"),
    ("docs/godot/godot_docs_stable_47_clean.md", "resource preloader", "rank1", "ResourcePreloader"),
    ("docs/godot/godot_docs_stable_47_clean.md", "TileSet scenes collection source", "top3", "TileSetScenesCollectionSource"),
    ("docs/godot/godot_docs_stable_47_clean.md", "Input is_action_just_pressed", "rank1", "is_action_just_pressed"),
    # --- Blender ---
    ("docs/blender/Blender52LTSManual.md", "Extrude Individual Faces", "rank1", "Extrude Individual Faces"),
    ("docs/blender/Blender52LTSManual.md", "clay thumb sculpt", "rank1", "Clay Thumb"),
    ("docs/blender/Blender52LTSManual.md", "integer vector node", "rank1", "Integer Vector Node"),
    ("docs/blender/Blender52LTSManual.md", "weight paint mask", "rank1", "Weight Paint"),
    ("docs/blender/Blender52LTSManual.md", "boolean modifier solver", "rank1", "Solver Options"),
    # --- Roblox API ---
    ("docs/roblox/roblox_engine_api.md", "TeleportService TeleportAsync", "rank1", "TeleportService:TeleportAsync"),
    ("docs/roblox/roblox_engine_api.md", "Players CreateHumanoidModelFromDescription", "rank1", "Players:CreateHumanoidModelFromDescription"),
    ("docs/roblox/roblox_engine_api.md", "RotationCurve SetKeys", "rank1", "RotationCurve:SetKeys"),
    ("docs/roblox/roblox_engine_api.md", "Humanoid MoveTo", "rank1", "Humanoid"),
    # --- Roblox docs ---
    ("docs/roblox/roblox_docs.md", "global chat commands", "rank1", "Global chat commands"),
    ("docs/roblox/roblox_docs_clean.md", "counter cyberbullying", "rank1", "Counter cyberbullying"),
    ("docs/roblox/roblox_docs_clean.md", "global chat commands", "rank1", "Global chat commands"),
]

for word in GODOT_LOST_WORDS:
    CASES.append(("docs/godot/godot_docs_stable_47.md", word, "found", None))

SECTION_RE = re.compile(r"--- DÉBUT DE SECTION.*?--- FIN DE SECTION", re.S)


def run_query(binary: Path, corpus: Path, query: str):
    proc = subprocess.run(
        [str(binary), str(corpus), query, "800", "3"],
        capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    sections = [m.group(0) for m in SECTION_RE.finditer(proc.stdout)]
    return proc.returncode, sections


def check(expectation, substr, code, sections):
    if expectation == "found":
        return code == 0, "exit 0" if code == 0 else f"exit {code}"
    if code != 0:
        return False, f"exit {code}"
    pool = sections[:3] if expectation == "top3" else sections[:1]
    hay = "\n".join(pool).lower()
    if substr.lower() in hay:
        return True, f"{expectation} ok"
    got = sections[0].splitlines()[1 if len(sections[0].splitlines()) > 1 else 0][:60]
    return False, f"no '{substr}' in {expectation}; rank1: {got!r}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=None, help="path to the md-doc-search binary")
    ap.add_argument("--only", default=None, help="run only cases whose corpus/query matches this substring")
    args = ap.parse_args()

    binary = Path(args.bin) if args.bin else REPO / "target" / "release" / (
        "md-doc-search.exe" if sys.platform == "win32" else "md-doc-search")
    if not binary.exists():
        sys.exit(f"binary not found: {binary} (run cargo build --release)")

    failures = skipped = passed = 0
    for corpus_rel, query, expectation, substr in CASES:
        if args.only and args.only.lower() not in f"{corpus_rel} {query}".lower():
            continue
        corpus = REPO / corpus_rel
        if not corpus.exists():
            print(f"  SKIP  {corpus_rel:45} {query!r} (file missing)")
            skipped += 1
            continue
        code, sections = run_query(binary, corpus, query)
        ok, detail = check(expectation, substr, code, sections)
        mark = "PASS" if ok else "FAIL"
        print(f"  {mark}  {corpus_rel:45} {query!r} -> {expectation}: {detail}")
        passed, failures = (passed + ok, failures + (not ok))

    total = passed + failures
    print(f"\n{passed}/{total} passed, {failures} failed, {skipped} skipped")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
