#!/usr/bin/env python3
"""Copy the pinned stable-diffusion.cpp commit's built-in recipes into catalog/sd-recipes.json.

    python3 -I scripts/vendor_sd_recipes.py <checkout> [--commit SHA] [--out PATH]

Reads the recipe files with `git show <commit>:<path>`, never the working tree, so a
blobless clone works. Writes {"sdCommit": ..., "recipes": [...]} sorted by recipe id.
Standard library only.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RECIPES_DIR = "examples/server/recipes/"


def git(checkout, *args):
    return subprocess.run(
        ["git", "-C", str(checkout), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout


def vendor(checkout, commit):
    paths = git(checkout, "ls-tree", "--name-only", commit, RECIPES_DIR).split("\n")
    recipes = [
        json.loads(git(checkout, "show", f"{commit}:{p}"))
        for p in paths
        if p.endswith(".json")
    ]
    recipes.sort(key=lambda r: r["id"])
    return {"sdCommit": commit, "recipes": recipes}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("checkout")
    ap.add_argument("--commit")
    ap.add_argument("--out", default=str(ROOT / "catalog" / "sd-recipes.json"))
    args = ap.parse_args(argv)
    commit = args.commit
    if commit is None:
        lock = json.loads((ROOT / "sd.lock.json").read_text(encoding="utf-8"))
        commit = lock["sd_cpp"]["commit"]
    doc = vendor(args.checkout, commit)
    Path(args.out).write_text(
        json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {len(doc['recipes'])} recipes for {commit}")


if __name__ == "__main__":
    sys.exit(main())
