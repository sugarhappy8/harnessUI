#!/usr/bin/env python3
"""Tests for scripts/vendor_sd_recipes.py and the vendored catalog/sd-recipes.json.

    python3 -I scripts/vendor_sd_recipes_test.py

Standard library only.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
SCRIPT = HERE / "vendor_sd_recipes.py"

A = {"schema_version": 1, "id": "a-recipe", "name": "A", "slots": [], "defaults": {"steps": 20}}
B = {"schema_version": 1, "id": "b-recipe", "name": "B ü", "slots": [], "defaults": {}}


def git(cwd, *args):
    return subprocess.run(
        ["git", "-C", str(cwd), "-c", "user.name=t", "-c", "user.email=t@example.com", *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def make_repo(tmp):
    repo = Path(tmp) / "engine"
    rdir = repo / "examples" / "server" / "recipes"
    rdir.mkdir(parents=True)
    git(repo, "init", "-q")
    # Written out of id order, to see the sorting.
    (rdir / "b.json").write_text(json.dumps(B), encoding="utf-8")
    (rdir / "a.json").write_text(json.dumps(A), encoding="utf-8")
    (rdir / "README.md").write_text("# recipes\n", encoding="utf-8")
    (rdir / "defaults.md").write_text("defaults\n", encoding="utf-8")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "recipes")
    return repo, git(repo, "rev-parse", "HEAD")


def run(repo, sha, out):
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPT), str(repo), "--commit", sha, "--out", str(out)],
        check=True,
        capture_output=True,
        text=True,
    )


class VendorTest(unittest.TestCase):
    def test_writes_every_recipe_of_the_commit_sorted(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, sha = make_repo(tmp)
            out = Path(tmp) / "out.json"
            run(repo, sha, out)
            doc = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(doc["sdCommit"], sha)
            self.assertEqual([r["id"] for r in doc["recipes"]], ["a-recipe", "b-recipe"])
            self.assertEqual(doc["recipes"], [A, B])
            self.assertNotIn("README", out.read_text(encoding="utf-8"))

    def test_reads_the_commit_not_the_working_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, sha = make_repo(tmp)
            path = repo / "examples" / "server" / "recipes" / "a.json"
            path.write_text(json.dumps({**A, "name": "changed"}), encoding="utf-8")
            out = Path(tmp) / "out.json"
            run(repo, sha, out)
            doc = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(doc["recipes"][0], A)

    def test_the_vendored_file_matches_the_lock(self):
        with open(ROOT / "catalog" / "sd-recipes.json", encoding="utf-8") as f:
            vendored = json.load(f)
        with open(ROOT / "sd.lock.json", encoding="utf-8") as f:
            lock = json.load(f)
        self.assertEqual(vendored["sdCommit"], lock["sd_cpp"]["commit"])
        self.assertEqual(len(vendored["recipes"]), 112)


if __name__ == "__main__":
    unittest.main()
